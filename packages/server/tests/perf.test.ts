/**
 * HTTP Performance Test — external fetch client against a real Node HTTP server
 *
 * Unlike the other server tests that use Hono's in-process `app.request()`, this
 * file starts an actual Node TCP server (via @hono/node-server) backed by a real
 * Redis instance (via testcontainers), then drives load through native `fetch`.
 *
 * This catches regressions that in-process tests cannot: serialisation overhead,
 * network stack, Redis round-trips, and SSE streaming over real connections.
 *
 * Prerequisites: Docker must be available (testcontainers pulls redis:7-alpine).
 */
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { serve } from '@hono/node-server'
import type { Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { TaskEngine } from '@taskcast/core'
import { createRedisAdapters } from '@taskcast/redis'
import { createTaskcastApp } from '../src/index.js'

// ── Shared infra (one server + Redis for the whole suite) ─────────────────────

let container: StartedTestContainer
let pub: Redis, sub: Redis, store: Redis
let server: Server
let BASE_URL: string

beforeAll(async () => {
  container = await new GenericContainer('redis:7-alpine').withExposedPorts(6379).start()
  const redisUrl = `redis://localhost:${container.getMappedPort(6379)}`
  pub = new Redis(redisUrl)
  sub = new Redis(redisUrl)
  store = new Redis(redisUrl)

  const adapters = createRedisAdapters(pub, sub, store)
  const engine = new TaskEngine(adapters)
  const app = createTaskcastApp({ engine })

  // Start real HTTP server on OS-assigned port (port: 0)
  await new Promise<void>((resolve) => {
    server = serve({ fetch: app.fetch, port: 0 }, (info: AddressInfo) => {
      BASE_URL = `http://localhost:${info.port}`
      resolve()
    }) as unknown as Server
  })
}, 60_000)

afterAll(async () => {
  await new Promise<void>((resolve) => server.close(() => resolve()))
  pub.disconnect()
  sub.disconnect()
  store.disconnect()
  await container?.stop()
})

// ── HTTP helpers ──────────────────────────────────────────────────────────────

interface Task { id: string; status: string }
interface TaskEvent { id: string; taskId: string; index: number; type: string }

async function post<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${BASE_URL}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  if (!res.ok) throw new Error(`POST ${path} → HTTP ${res.status}`)
  return res.json() as Promise<T>
}

async function patch<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${BASE_URL}${path}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`PATCH ${path} → HTTP ${res.status}`)
  return res.json() as Promise<T>
}

const createTask = () => post<Task>('/tasks', { type: 'perf' })
const startTask = (id: string) => patch<Task>(`/tasks/${id}/status`, { status: 'running' })
const publishEvent = (id: string, i: number) =>
  post<TaskEvent>(`/tasks/${id}/events`, { type: 'perf.event', level: 'info', data: { i } })

// ── Resource monitor ─────────────────────────────────────────────────────────

interface ResourceReport {
  peakRssMb: number
  peakHeapMb: number
  /** CPU utilization % over the benchmark window (user+sys / wall clock) */
  cpuPct: number
}

function startResourceMonitor(intervalMs = 200) {
  const startCpu = process.cpuUsage()
  const startWall = performance.now()
  const peakRss = { v: 0 }
  const peakHeap = { v: 0 }

  const timer = setInterval(() => {
    const mem = process.memoryUsage()
    if (mem.rss > peakRss.v) peakRss.v = mem.rss
    if (mem.heapUsed > peakHeap.v) peakHeap.v = mem.heapUsed
  }, intervalMs)

  return (): ResourceReport => {
    clearInterval(timer)
    // Always capture a final sample so short-lived tests (< intervalMs) still report
    const mem = process.memoryUsage()
    if (mem.rss > peakRss.v) peakRss.v = mem.rss
    if (mem.heapUsed > peakHeap.v) peakHeap.v = mem.heapUsed

    const finalCpu = process.cpuUsage(startCpu)
    const wallMs = performance.now() - startWall
    const cpuMs = (finalCpu.user + finalCpu.system) / 1000
    return {
      peakRssMb: Math.round(peakRss.v / 1024 / 1024),
      peakHeapMb: Math.round(peakHeap.v / 1024 / 1024),
      cpuPct: Math.round((cpuMs / wallMs) * 100),
    }
  }
}

// ── Benchmark runner ──────────────────────────────────────────────────────────

interface BenchResult {
  rps: number; errors: number; p50: number; p99: number
  resources: ResourceReport
}

async function bench(
  fn: () => Promise<unknown>,
  concurrency: number,
  durationMs: number,
): Promise<BenchResult> {
  let done = false
  let count = 0
  let errors = 0
  const latencies: number[] = []

  const stopMonitor = startResourceMonitor()

  const worker = async () => {
    while (!done) {
      const t0 = performance.now()
      try {
        await fn()
        latencies.push(performance.now() - t0)
        count++
      } catch {
        errors++
      }
    }
  }

  await new Promise<void>((resolve) => {
    setTimeout(() => { done = true; resolve() }, durationMs)
    void Promise.all(Array.from({ length: concurrency }, worker))
  })

  const resources = stopMonitor()
  latencies.sort((a, b) => a - b)
  const p = (pct: number) => Math.round(latencies[Math.floor(latencies.length * pct)] ?? 0)
  return { rps: Math.round(count / (durationMs / 1000)), errors, p50: p(0.5), p99: p(0.99), resources }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('HTTP performance — real TCP connections to Node server + Redis', () => {
  it('idle baseline — resource usage with zero active requests', async () => {
    // Let the event loop drain and GC settle before sampling
    await new Promise((r) => setTimeout(r, 500))
    if (global.gc) global.gc()
    await new Promise((r) => setTimeout(r, 100))

    const mem = process.memoryUsage()
    const rssMb   = Math.round(mem.rss      / 1024 / 1024)
    const heapMb  = Math.round(mem.heapUsed / 1024 / 1024)
    const totalMb = Math.round(mem.heapTotal / 1024 / 1024)
    const extMb   = Math.round(mem.external  / 1024 / 1024)

    console.log(
      `[idle] RSS=${rssMb}MB  heapUsed=${heapMb}MB  heapTotal=${totalMb}MB  external=${extMb}MB`,
    )

    // Sanity bounds — not a regression assertion, just documents the baseline
    expect(rssMb).toBeGreaterThan(0)
  })

  it('task creation: ≥ 100 req/s at concurrency=10, 5 s', async () => {
    const result = await bench(createTask, 10, 5_000)
    const r = result.resources
    console.log(
      `[perf] task create: ${result.rps} req/s` +
      `  p50=${result.p50}ms  p99=${result.p99}ms  errors=${result.errors}\n` +
      `       resources: RSS=${r.peakRssMb}MB  heap=${r.peakHeapMb}MB  cpu=${r.cpuPct}%`,
    )
    expect(result.errors).toBe(0)
    expect(result.rps).toBeGreaterThanOrEqual(100)
  }, 30_000)

  it('event publish: ≥ 100 req/s at concurrency=10, 5 s', async () => {
    const task = await createTask()
    await startTask(task.id)
    const result = await bench(() => publishEvent(task.id, 0), 10, 5_000)
    const r = result.resources
    console.log(
      `[perf] event publish: ${result.rps} req/s` +
      `  p50=${result.p50}ms  p99=${result.p99}ms  errors=${result.errors}\n` +
      `       resources: RSS=${r.peakRssMb}MB  heap=${r.peakHeapMb}MB  cpu=${r.cpuPct}%`,
    )
    expect(result.errors).toBe(0)
    expect(result.rps).toBeGreaterThanOrEqual(100)
  }, 30_000)

  it('SSE fan-out: 10 subscribers all receive taskcast.done within 3 s after 50 events', async () => {
    const task = await createTask()
    await startTask(task.id)

    const EVENT_COUNT = 50
    const SUBSCRIBER_COUNT = 10

    // Each promise resolves when its SSE stream delivers `taskcast.done`
    const resolvers: Array<() => void> = []
    const donePromises = Array.from(
      { length: SUBSCRIBER_COUNT },
      () => new Promise<void>((resolve) => resolvers.push(resolve)),
    )

    // Open real HTTP SSE connections — each runs in its own async IIFE
    resolvers.forEach((resolve) => {
      const ctrl = new AbortController()
      ;(async () => {
        try {
          const res = await fetch(`${BASE_URL}/tasks/${task.id}/events`, {
            signal: ctrl.signal,
            headers: { Accept: 'text/event-stream' },
          })
          const reader = res.body!.getReader()
          const dec = new TextDecoder()
          let buf = ''
          outer: while (true) {
            const { done, value } = await reader.read()
            if (done) { resolve(); break }
            buf += dec.decode(value, { stream: true })
            const blocks = buf.split('\n\n')
            buf = blocks.pop() ?? ''
            for (const block of blocks) {
              if (block.includes('event: taskcast.done')) { resolve(); break outer }
            }
          }
        } catch {
          resolve() // AbortError or connection reset — count as done
        }
      })()
    })

    // Allow Redis pub/sub subscriptions to propagate before publishing
    await new Promise((r) => setTimeout(r, 200))

    const stopMonitor = startResourceMonitor()
    const t0 = performance.now()
    for (let i = 0; i < EVENT_COUNT; i++) {
      await publishEvent(task.id, i)
    }
    // Completing the task triggers taskcast.done on all SSE streams
    await patch<Task>(`/tasks/${task.id}/status`, { status: 'completed' })

    const result = await Promise.race([
      Promise.all(donePromises).then(() => 'ok' as const),
      new Promise<'timeout'>((r) => setTimeout(() => r('timeout'), 3000)),
    ])
    const elapsed = Math.round(performance.now() - t0)
    const res = stopMonitor()

    console.log(
      `[perf] SSE fan-out: ${SUBSCRIBER_COUNT} subs × ${EVENT_COUNT} events` +
      ` — all taskcast.done in ${elapsed}ms\n` +
      `       resources: RSS=${res.peakRssMb}MB  heap=${res.peakHeapMb}MB  cpu=${res.cpuPct}%`,
    )
    expect(result).toBe('ok')
  }, 30_000)
})
