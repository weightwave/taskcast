#!/usr/bin/env node
/**
 * Taskcast Performance Comparison: Node.js vs Rust
 *
 * Starts each 3-instance stack sequentially, runs the same benchmark suite,
 * then prints a side-by-side comparison table.
 *
 * Prerequisites (build images first):
 *   docker compose -f docker-compose.stress.yml build
 *   docker compose -f docker-compose.stress-rust.yml build
 *
 * Run:
 *   npx tsx scripts/compare.ts
 */

import { exec } from 'node:child_process'
import { execSync } from 'node:child_process'
import { promisify } from 'node:util'

const execAsync = promisify(exec)
const BASE_URL = 'http://localhost:8080'
const BENCH_CONCURRENCY = 10
const WARMUP_MS = 10_000
const BENCH_MS = 10_000

// ── ANSI helpers ──────────────────────────────────────────────────────────────

const C = {
  bold: '\x1b[1m', reset: '\x1b[0m', dim: '\x1b[2m',
  green: '\x1b[32m', red: '\x1b[31m', yellow: '\x1b[33m', cyan: '\x1b[36m',
}
const bold  = (s: string) => `${C.bold}${s}${C.reset}`
const dim   = (s: string) => `${C.dim}${s}${C.reset}`
const green = (s: string) => `${C.green}${s}${C.reset}`
const red   = (s: string) => `${C.red}${s}${C.reset}`

// Strip ANSI codes to compute visible length for column alignment
const visLen = (s: string) => s.replace(/\x1b\[[^m]*m/g, '').length
const padEnd  = (s: string, n: number) => s + ' '.repeat(Math.max(0, n - visLen(s)))

// ── HTTP helpers ──────────────────────────────────────────────────────────────

interface Task  { id: string; status: string }
interface Event { id: string; taskId: string; index: number; type: string }

async function http<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${BASE_URL}${path}`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  if (!res.ok) throw new Error(`${method} ${path} → HTTP ${res.status}`)
  return res.json() as Promise<T>
}

const POST  = <T>(path: string, body?: unknown) => http<T>('POST', path, body)
const PATCH = <T>(path: string, body: unknown)  => http<T>('PATCH', path, body)

const createTask   = () => POST<Task>('/tasks', { type: 'compare' })
const startTask    = (id: string) => PATCH<Task>(`/tasks/${id}/status`, { status: 'running' })
const completeTask = (id: string) => PATCH<Task>(`/tasks/${id}/status`, { status: 'completed' })
const publishEvent = (id: string) =>
  POST<Event>(`/tasks/${id}/events`, { type: 'compare.event', level: 'info', data: null })

// ── Benchmark runner ──────────────────────────────────────────────────────────

interface BenchResult { rps: number; errors: number; p50: number; p99: number; p999: number }

async function bench(
  fn: () => Promise<unknown>,
  concurrency: number,
  durationMs: number,
): Promise<BenchResult> {
  let done = false
  let count = 0
  let errors = 0
  const latencies: number[] = []

  const worker = async () => {
    while (!done) {
      const t0 = performance.now()
      try { await fn(); latencies.push(performance.now() - t0); count++ }
      catch { errors++ }
    }
  }

  await new Promise<void>((resolve) => {
    setTimeout(() => { done = true; resolve() }, durationMs)
    void Promise.all(Array.from({ length: concurrency }, worker))
  })

  latencies.sort((a, b) => a - b)
  const p = (pct: number) => Math.round(latencies[Math.floor(latencies.length * pct)] ?? 0)
  return { rps: Math.round(count / (durationMs / 1000)), errors, p50: p(0.5), p99: p(0.99), p999: p(0.999) }
}

// ── Docker stats ──────────────────────────────────────────────────────────────

interface DockerStats { cpuAvgPct: number; memPeakMb: number }

async function sampleDockerStats(nameFilter: string): Promise<{ cpu: number; memMb: number }> {
  try {
    const { stdout } = await execAsync(
      `docker stats --no-stream --format "{{.CPUPerc}}\t{{.MemUsage}}" --filter "name=${nameFilter}"`,
      { timeout: 5000 },
    )
    let totalCpu = 0
    let cpuCount = 0
    let totalMemMb = 0
    for (const line of stdout.trim().split('\n')) {
      if (!line.trim()) continue
      const [cpuStr, memStr] = line.split('\t')
      const cpu = parseFloat(cpuStr?.replace('%', '') ?? '')
      if (!isNaN(cpu)) { totalCpu += cpu; cpuCount++ }
      // memStr format: "123.4MiB / 512.0MiB" — take the used (first) value
      const m = memStr?.match(/([\d.]+)(GiB|MiB|GB|MB|kB|B)/)
      if (m) {
        let mb = parseFloat(m[1])
        if (m[2] === 'GiB' || m[2] === 'GB') mb *= 1024
        if (m[2] === 'kB') mb /= 1024
        if (m[2] === 'B')  mb /= 1_048_576
        totalMemMb += mb
      }
    }
    return { cpu: cpuCount > 0 ? totalCpu / cpuCount : 0, memMb: totalMemMb }
  } catch {
    return { cpu: 0, memMb: 0 }
  }
}

async function collectDockerStatsDuring(nameFilter: string, durationMs: number): Promise<DockerStats> {
  const cpuSamples: number[] = []
  let peakMemMb = 0
  let active = true

  const loop = async () => {
    while (active) {
      const s = await sampleDockerStats(nameFilter)
      if (s.cpu > 0) cpuSamples.push(s.cpu)
      if (s.memMb > peakMemMb) peakMemMb = s.memMb
      await new Promise((r) => setTimeout(r, 500))
    }
  }

  const loopDone = loop()
  await new Promise((r) => setTimeout(r, durationMs))
  active = false
  await loopDone

  const cpuAvgPct = cpuSamples.length > 0
    ? Math.round(cpuSamples.reduce((a, b) => a + b, 0) / cpuSamples.length)
    : 0
  return { cpuAvgPct, memPeakMb: Math.round(peakMemMb) }
}

// ── Stack lifecycle ───────────────────────────────────────────────────────────

function dockerCompose(file: string, args: string) {
  execSync(`docker compose -f ${file} ${args}`, { stdio: 'inherit' })
}

/** Poll until the LB is forwarding requests to a backend. */
async function waitForLB(timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      // Any non-5xx (or non-network-error) response means nginx + a backend are up
      const res = await fetch(`${BASE_URL}/tasks/__probe__`, {
        signal: AbortSignal.timeout(2000),
      })
      if (res.status < 500) return
    } catch { /* not ready yet */ }
    await new Promise((r) => setTimeout(r, 500))
  }
  throw new Error(`LB at ${BASE_URL} did not become ready within ${timeoutMs}ms`)
}

// ── SSE fan-out test ──────────────────────────────────────────────────────────

async function sseFanOut(subscriberCount: number, eventCount: number): Promise<number> {
  const task = await createTask()
  await startTask(task.id)

  const resolvers: Array<() => void> = []
  const donePromises = Array.from(
    { length: subscriberCount },
    () => new Promise<void>((resolve) => resolvers.push(resolve)),
  )

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
            if (block.includes('event: taskcast.done')) { ctrl.abort(); resolve(); break outer }
          }
        }
      } catch { resolve() }
    })()
  })

  // Allow Redis pub/sub subscriptions to propagate before publishing
  await new Promise((r) => setTimeout(r, 200))

  const t0 = performance.now()
  for (let i = 0; i < eventCount; i++) await publishEvent(task.id)
  await completeTask(task.id)

  const result = await Promise.race([
    Promise.all(donePromises).then(() => 'ok' as const),
    new Promise<'timeout'>((r) => setTimeout(() => r('timeout'), 8000)),
  ])
  if (result === 'timeout') throw new Error('SSE fan-out timed out')
  return Math.round(performance.now() - t0)
}

// ── Full benchmark suite ──────────────────────────────────────────────────────

interface StackResult {
  name: string
  create: BenchResult
  publish: BenchResult
  sseFanOutMs: number
  docker: DockerStats
}

async function runSuite(
  label: string,
  composeFile: string,
  dockerFilter: string,
): Promise<StackResult> {
  console.log(`\n${bold(`▶ ${label}`)}`)

  // 1. Start stack
  console.log(dim('  Starting stack (docker compose up -d --wait)...'))
  dockerCompose(composeFile, 'up -d --wait')
  await waitForLB()
  console.log(dim('  Stack ready.'))

  // 2. Warmup — gives Node.js time to JIT-compile hot paths
  console.log(dim(`  Warming up for ${WARMUP_MS / 1000}s...`))
  await bench(createTask, BENCH_CONCURRENCY, WARMUP_MS)

  // 3. Task creation benchmark + docker stats in parallel
  console.log(dim('  Benchmarking task creation...'))
  const [create, createStats] = await Promise.all([
    bench(createTask, BENCH_CONCURRENCY, BENCH_MS),
    collectDockerStatsDuring(dockerFilter, BENCH_MS),
  ])
  console.log(
    `  create:  ${bold(String(create.rps))} req/s` +
    `  p50=${create.p50}ms  p99=${create.p99}ms  p999=${create.p999}ms  errors=${create.errors}`,
  )

  // 4. Event publish benchmark + docker stats in parallel
  console.log(dim('  Benchmarking event publish...'))
  const benchTask = await createTask()
  await startTask(benchTask.id)
  const [publish, publishStats] = await Promise.all([
    bench(() => publishEvent(benchTask.id), BENCH_CONCURRENCY, BENCH_MS),
    collectDockerStatsDuring(dockerFilter, BENCH_MS),
  ])
  console.log(
    `  publish: ${bold(String(publish.rps))} req/s` +
    `  p50=${publish.p50}ms  p99=${publish.p99}ms  p999=${publish.p999}ms  errors=${publish.errors}`,
  )

  // 5. SSE fan-out (10 subscribers × 30 events)
  console.log(dim('  Testing SSE fan-out (10 subs × 30 events)...'))
  let sseFanOutMs: number
  try {
    sseFanOutMs = await sseFanOut(10, 30)
    console.log(`  SSE fan-out: ${bold(`${sseFanOutMs}ms`)} for all 10 subscribers to receive taskcast.done`)
  } catch (err) {
    sseFanOutMs = -1
    console.log(`  ${C.red}SSE fan-out FAILED: ${err}${C.reset}`)
  }

  // 6. Combine docker stats
  const docker: DockerStats = {
    cpuAvgPct: Math.round((createStats.cpuAvgPct + publishStats.cpuAvgPct) / 2),
    memPeakMb: Math.max(createStats.memPeakMb, publishStats.memPeakMb),
  }
  console.log(
    `  docker:  CPU avg ${docker.cpuAvgPct}%  memory peak ${docker.memPeakMb}MB` +
    dim(' (3 taskcast instances combined)'),
  )

  // 7. Stop stack
  console.log(dim('  Stopping stack...'))
  dockerCompose(composeFile, 'down')

  return { name: label, create, publish, sseFanOutMs, docker }
}

// ── Comparison table ──────────────────────────────────────────────────────────

function printComparison(a: StackResult, b: StackResult) {
  // Returns [aFormatted, bFormatted] with green for winner, red for loser
  const winner = (aVal: number, bVal: number, higherIsBetter: boolean): [string, string] => {
    const aWins = higherIsBetter ? aVal >= bVal : aVal <= bVal
    if (aVal === bVal) return [String(aVal), String(bVal)]
    return [
      aWins ? green(String(aVal)) : red(String(aVal)),
      aWins ? red(String(bVal)) : green(String(bVal)),
    ]
  }

  const LABEL_W = 26
  const COL_W   = 16
  const sep = '─'.repeat(LABEL_W + COL_W * 2 + 4)

  const row = (label: string, aVal: string, bVal: string) => {
    console.log(`  ${label.padEnd(LABEL_W)} ${padEnd(aVal, COL_W)} ${padEnd(bVal, COL_W)}`)
  }

  console.log(`\n${bold('═══ Node.js vs Rust — Performance Comparison ═══')}`)
  console.log(dim(sep))
  console.log(`  ${'Metric'.padEnd(LABEL_W)} ${a.name.padEnd(COL_W)} ${b.name.padEnd(COL_W)}`)
  console.log(dim(sep))

  const [aCreate, bCreate] = winner(a.create.rps, b.create.rps, true)
  row('Task creation (req/s)', aCreate, bCreate)
  row('  p50 (ms)',  ...winner(a.create.p50,  b.create.p50,  false))
  row('  p99 (ms)',  ...winner(a.create.p99,  b.create.p99,  false))
  row('  p999 (ms)', ...winner(a.create.p999, b.create.p999, false))

  console.log(dim('  ' + '·'.repeat(sep.length - 2)))

  const [aPublish, bPublish] = winner(a.publish.rps, b.publish.rps, true)
  row('Event publish (req/s)', aPublish, bPublish)
  row('  p50 (ms)',  ...winner(a.publish.p50,  b.publish.p50,  false))
  row('  p99 (ms)',  ...winner(a.publish.p99,  b.publish.p99,  false))
  row('  p999 (ms)', ...winner(a.publish.p999, b.publish.p999, false))

  console.log(dim('  ' + '·'.repeat(sep.length - 2)))

  const sseA = a.sseFanOutMs >= 0 ? String(a.sseFanOutMs) + 'ms' : 'FAILED'
  const sseB = b.sseFanOutMs >= 0 ? String(b.sseFanOutMs) + 'ms' : 'FAILED'
  if (a.sseFanOutMs >= 0 && b.sseFanOutMs >= 0) {
    row('SSE fan-out (ms)', ...winner(a.sseFanOutMs, b.sseFanOutMs, false))
  } else {
    row('SSE fan-out (ms)', sseA, sseB)
  }

  console.log(dim('  ' + '·'.repeat(sep.length - 2)))

  row('CPU avg % (3 instances)', ...winner(a.docker.cpuAvgPct, b.docker.cpuAvgPct, false).map((v) => v + '%') as [string, string])
  row('Memory peak (3 instances)', ...winner(a.docker.memPeakMb, b.docker.memPeakMb, false).map((v) => v + ' MB') as [string, string])

  console.log(dim(sep))
  console.log(dim(`  Green = winner per metric  |  Concurrency: ${BENCH_CONCURRENCY}  Bench: ${BENCH_MS / 1000}s each`))
  console.log()
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  console.log(`\n${bold('═══ Taskcast Node.js vs Rust Performance Comparison ═══')}`)
  console.log(dim(`Stack: 3× taskcast + Redis + nginx  |  Concurrency: ${BENCH_CONCURRENCY}  Warmup: ${WARMUP_MS / 1000}s  Bench: ${BENCH_MS / 1000}s`))
  console.log(dim('Both stacks run sequentially on port 8080'))
  console.log()

  const nodeResult = await runSuite(
    'Node.js (3×)',
    'docker-compose.stress.yml',
    'taskcast-stress-taskcast',
  )

  const rustResult = await runSuite(
    'Rust (3×)',
    'docker-compose.stress-rust.yml',
    'taskcast-stress-rust-taskcast',
  )

  printComparison(nodeResult, rustResult)
}

main().catch((err) => { console.error(err); process.exit(1) })
