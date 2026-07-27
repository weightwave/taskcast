import { spawn, type ChildProcess } from 'node:child_process'
import { createServer } from 'node:net'
import { resolve } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  TaskEngine,
} from '../../packages/core/src/index.js'
import { createTaskcastApp } from '../../packages/server/src/index.js'

type ApiResult = {
  status: number
  data: unknown
  text: string
}

const repoRoot = resolve(import.meta.dirname, '../..')
let rust: ChildProcess
let rustBaseUrl: string
let tsApp: ReturnType<typeof createTaskcastApp>

async function availablePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      if (!address || typeof address === 'string') {
        reject(new Error('failed to allocate parity port'))
        return
      }
      server.close(() => resolvePort(address.port))
    })
  })
}

async function parseResponse(response: Response): Promise<ApiResult> {
  const text = await response.text()
  let data: unknown = null
  if (text !== '') {
    try {
      data = JSON.parse(text)
    } catch {
      data = null
    }
  }
  return {
    status: response.status,
    data,
    text,
  }
}

async function rustApi(path: string, init?: RequestInit): Promise<ApiResult> {
  return parseResponse(await fetch(`${rustBaseUrl}${path}`, init))
}

async function tsApi(path: string, init?: RequestInit): Promise<ApiResult> {
  return parseResponse(await tsApp.app.request(`http://taskcast.test${path}`, init))
}

async function waitForRust(): Promise<void> {
  for (let attempt = 0; attempt < 240; attempt++) {
    if (rust.exitCode !== null) {
      throw new Error(`Rust parity server exited with ${rust.exitCode}`)
    }
    try {
      if ((await rustApi('/health')).status === 200) return
    } catch {
      // The build or listener is still starting.
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 250))
  }
  throw new Error('Rust parity server did not become ready')
}

const json = (body: unknown): RequestInit => ({
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

function canonicalEvents(value: unknown): unknown[] {
  return (value as Array<Record<string, unknown>>).map((event) => ({
    index: event['index'],
    type: event['type'],
    level: event['level'],
    data: event['data'],
    seriesId: event['seriesId'],
    seriesMode: event['seriesMode'],
    seriesAccField: event['seriesAccField'],
    seriesSnapshot: event['seriesSnapshot'],
  }))
}

async function runLifecycle(
  api: (path: string, init?: RequestInit) => Promise<ApiResult>,
  taskId: string,
) {
  expect((await api('/tasks', json({
    id: taskId,
    type: 'agent.session',
  }))).status).toBe(201)
  const keep = await api(`/tasks/${taskId}/events`, json({
    type: 'agent.retry',
    level: 'warn',
    data: { attempt: 1 },
  }))
  const delta = await api(`/tasks/${taskId}/events`, json({
    type: 'agent.output',
    level: 'info',
    data: { delta: 'A' },
    seriesId: 'output',
    seriesMode: 'accumulate',
    seriesAccField: 'delta',
  }))
  await api(`/tasks/${taskId}/events`, json({
    type: 'agent.output',
    level: 'info',
    data: { delta: 'B' },
    seriesId: 'output',
    seriesMode: 'accumulate',
    seriesAccField: 'delta',
  }))
  const before = await api(`/tasks/${taskId}/events/history?seriesFormat=delta`)
  const last = (before.data as Array<Record<string, number>>).at(-1)!
  const release = await api(`/tasks/${taskId}/storage/release`, json({
    expectedLastEventIndex: last['index'],
    inactiveSince: Math.max(
      (keep.data as Record<string, number>)['timestamp']!,
      (delta.data as Record<string, number>)['timestamp']!,
      Date.now(),
    ),
  }))
  const cold = await api(`/tasks/${taskId}/events/history?seriesFormat=delta`)
  const late = await api(`/tasks/${taskId}/events`, json({
    type: 'agent.owner_reacquired',
    level: 'info',
    data: { resumed: true },
  }))
  const cursor = await api(
    `/tasks/${taskId}/events/history?seriesFormat=delta&since.index=1`,
  )
  return { before, release, cold, late, cursor }
}

beforeAll(async () => {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const engine = new TaskEngine({
    shortTermStore: hot,
    longTermStore: durable,
    broadcast: new MemoryBroadcastProvider(),
  })
  tsApp = createTaskcastApp({
    engine,
    shortTermStore: hot,
  })

  const port = await availablePort()
  rustBaseUrl = `http://127.0.0.1:${port}`
  rust = spawn(
    'cargo',
    [
      'run',
      '--quiet',
      '-p',
      'taskcast-server',
      '--example',
      'hot_cold_parity_server',
    ],
    {
      cwd: resolve(repoRoot, 'rust'),
      env: {
        ...process.env,
        TASKCAST_PARITY_PORT: String(port),
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  )
  await waitForRust()
}, 120_000)

afterAll(() => {
  tsApp?.stop()
  rust?.kill('SIGTERM')
})

describe('TypeScript/Rust hot-cold HTTP parity', () => {
  it('matches release responses, canonical history, cursors, and later-write indexes', async () => {
    const [ts, rs] = await Promise.all([
      runLifecycle(tsApi, 'parity-task'),
      runLifecycle(rustApi, 'parity-task'),
    ])

    expect(rs.before.status).toBe(ts.before.status)
    expect(canonicalEvents(rs.before.data)).toEqual(canonicalEvents(ts.before.data))
    expect(rs.release.status).toBe(ts.release.status)
    expect(rs.release.data).toEqual(ts.release.data)
    expect(canonicalEvents(rs.cold.data)).toEqual(canonicalEvents(ts.cold.data))
    expect(rs.late.status).toBe(ts.late.status)
    expect((rs.late.data as Record<string, unknown>)['index']).toBe(
      (ts.late.data as Record<string, unknown>)['index'],
    )
    expect(canonicalEvents(rs.cursor.data)).toEqual(canonicalEvents(ts.cursor.data))
  }, 30_000)

  it('matches terminal SSE replay frames', async () => {
    const taskId = 'parity-terminal'
    for (const api of [tsApi, rustApi]) {
      await api('/tasks', json({ id: taskId }))
      await api(`/tasks/${taskId}/status`, {
        ...json({ status: 'cancelled' }),
        method: 'PATCH',
      })
    }
    const [ts, rs] = await Promise.all([
      tsApi(`/tasks/${taskId}/events`),
      rustApi(`/tasks/${taskId}/events`),
    ])
    expect(rs.status).toBe(ts.status)
    const normalizeFrames = (text: string) => text
      .split('\n\n')
      .map((frame) => frame.split('\n').filter((line) =>
        line.startsWith('event:') || line.startsWith('data:')
      ))
      .filter((frame) => frame.length > 0)
      .map((frame) => frame.map((line) => {
        if (!line.startsWith('data:')) return line
        const data = JSON.parse(line.slice(5).trim()) as Record<string, unknown>
        delete data['id']
        delete data['eventId']
        delete data['taskId']
        delete data['timestamp']
        return data
      }))
    expect(normalizeFrames(rs.text)).toEqual(normalizeFrames(ts.text))
  }, 30_000)
})
