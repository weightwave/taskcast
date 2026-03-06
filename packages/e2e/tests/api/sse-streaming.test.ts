import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'

// ─── SSE Helpers ──────────────────────────────────────────────────────────────

interface SSEMessage {
  event: string
  data: string
  id?: string
}

/**
 * Collects SSE messages from a fetch Response body.
 * Parses `event:`, `data:`, and `id:` lines separated by `\n\n`.
 * Resolves when the stream closes.
 */
async function collectSSE(
  response: Response,
  opts?: { signal?: AbortSignal },
): Promise<SSEMessage[]> {
  const messages: SSEMessage[] = []
  const reader = response.body!.getReader()
  const decoder = new TextDecoder()
  let buffer = ''

  try {
    // eslint-disable-next-line no-constant-condition
    while (true) {
      if (opts?.signal?.aborted) break
      const { done, value } = await reader.read()
      if (done) break

      buffer += decoder.decode(value, { stream: true })

      // Split on double newline (SSE message boundary)
      const parts = buffer.split('\n\n')
      // Last element is incomplete — keep it in the buffer
      buffer = parts.pop()!

      for (const part of parts) {
        if (!part.trim()) continue
        const msg: SSEMessage = { event: '', data: '' }
        for (const line of part.split('\n')) {
          if (line.startsWith('event:')) msg.event = line.slice(6).trim()
          else if (line.startsWith('data:')) msg.data = line.slice(5).trim()
          else if (line.startsWith('id:')) msg.id = line.slice(3).trim()
        }
        if (msg.event || msg.data) messages.push(msg)
      }
    }
  } catch (err) {
    // AbortError is expected when we cancel the stream
    if (!(err instanceof DOMException && err.name === 'AbortError')) throw err
  } finally {
    reader.releaseLock()
  }

  return messages
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('SSE Streaming API', () => {
  let server: TestServer

  beforeAll(async () => {
    server = await startServer()
  })

  afterAll(() => {
    server.close()
  })

  it('replays history and sends taskcast.done for a terminal task', async () => {
    // Create and run a task, publish events, then complete it
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'sse-history' }),
    })
    const { id } = await createRes.json()

    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    await fetch(`${server.baseUrl}/tasks/${id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'log', level: 'info', data: { msg: 'hello' } }),
    })

    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed', result: { ok: true } }),
    })

    // Now subscribe — task is already terminal, should replay + done
    const sseRes = await fetch(`${server.baseUrl}/tasks/${id}/events`)
    const messages = await collectSSE(sseRes)

    // Should have at least the event messages and a done event
    const eventMessages = messages.filter((m) => m.event === 'taskcast.event')
    const doneMessages = messages.filter((m) => m.event === 'taskcast.done')

    expect(eventMessages.length).toBeGreaterThanOrEqual(1)
    expect(doneMessages.length).toBe(1)

    const doneData = JSON.parse(doneMessages[0].data)
    expect(doneData.reason).toBe('completed')
  })

  it('streams live events and closes on terminal transition', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'sse-live' }),
    })
    const { id } = await createRes.json()

    // Transition to running before subscribing
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Start SSE stream
    const sseRes = await fetch(`${server.baseUrl}/tasks/${id}/events`)

    // Collect SSE in background — it will resolve when the stream closes
    const collectPromise = collectSSE(sseRes)

    // Give SSE a moment to establish
    await new Promise((r) => setTimeout(r, 50))

    // Publish a live event
    await fetch(`${server.baseUrl}/tasks/${id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'progress', level: 'info', data: { pct: 50 } }),
    })

    // Complete the task — should close the stream
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })

    const messages = await collectPromise

    const eventMessages = messages.filter((m) => m.event === 'taskcast.event')
    const doneMessages = messages.filter((m) => m.event === 'taskcast.done')

    // Should include replayed taskcast:status (running) + live progress + live taskcast:status (completed)
    expect(eventMessages.length).toBeGreaterThanOrEqual(2)

    // Should have a live progress event
    const progressEvents = eventMessages.filter((m) => {
      const d = JSON.parse(m.data)
      return d.type === 'progress'
    })
    expect(progressEvents.length).toBe(1)

    // Stream should have closed with done event
    expect(doneMessages.length).toBe(1)
    const doneData = JSON.parse(doneMessages[0].data)
    expect(doneData.reason).toBe('completed')
  })

  it('shows subscriberCount on task during active SSE subscription', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'sse-subscriber' }),
    })
    const { id } = await createRes.json()

    // Transition to running
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Before any subscriber
    const before = await fetch(`${server.baseUrl}/tasks/${id}`)
    const beforeTask = await before.json()
    expect(beforeTask.subscriberCount).toBe(0)
    expect(beforeTask.hot).toBe(false)

    // Open SSE subscription
    const controller = new AbortController()
    const sseRes = await fetch(`${server.baseUrl}/tasks/${id}/events`, {
      signal: controller.signal,
    })

    // Start reading in the background (but don't await)
    const collectPromise = collectSSE(sseRes, { signal: controller.signal }).catch(() => {})

    // Give SSE a moment to register the subscriber
    await new Promise((r) => setTimeout(r, 100))

    // Check subscriber count
    const during = await fetch(`${server.baseUrl}/tasks/${id}`)
    const duringTask = await during.json()
    expect(duringTask.subscriberCount).toBe(1)
    expect(duringTask.hot).toBe(true)

    // Abort the SSE connection
    controller.abort()
    await collectPromise

    // Give a moment for cleanup
    await new Promise((r) => setTimeout(r, 100))

    // Subscriber count should be back to 0
    const after = await fetch(`${server.baseUrl}/tasks/${id}`)
    const afterTask = await after.json()
    expect(afterTask.subscriberCount).toBe(0)
    expect(afterTask.hot).toBe(false)
  })
})
