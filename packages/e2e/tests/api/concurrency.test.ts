import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'
import { collectSSE } from '../helpers/sse.js'

describe('Concurrency API', () => {
  let server: TestServer

  beforeAll(async () => {
    server = await startServer()
  })

  afterAll(() => {
    server.close()
  })

  it('10 concurrent terminal transitions: only 1 succeeds (200), 9 get 400', async () => {
    // Create a task and move to running
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'concurrent-transition' }),
    })
    const { id } = await createRes.json()

    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Fire 10 concurrent transitions to completed
    const results = await Promise.all(
      Array.from({ length: 10 }, () =>
        fetch(`${server.baseUrl}/tasks/${id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'completed', result: { winner: true } }),
        }).then((r) => r.status),
      ),
    )

    const successes = results.filter((s) => s === 200)
    const failures = results.filter((s) => s === 400)

    expect(successes.length).toBe(1)
    expect(failures.length).toBe(9)
  })

  it('3 concurrent SSE subscribers all receive taskcast.done', async () => {
    // Create a task and move to running
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'concurrent-sse' }),
    })
    const { id } = await createRes.json()

    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Open 3 SSE connections
    const sseResponses = await Promise.all(
      Array.from({ length: 3 }, () =>
        fetch(`${server.baseUrl}/tasks/${id}/events`),
      ),
    )

    // Start collecting SSE messages for all 3
    const collectPromises = sseResponses.map((r) => collectSSE(r))

    // Give subscribers a moment to register
    await new Promise((r) => setTimeout(r, 100))

    // Complete the task
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })

    // Wait for all SSE streams to close
    const allMessages = await Promise.all(collectPromises)

    // Each subscriber should have received the taskcast.done event
    for (const messages of allMessages) {
      const doneMessages = messages.filter((m) => m.event === 'taskcast.done')
      expect(doneMessages.length).toBe(1)
      const doneData = JSON.parse(doneMessages[0].data)
      expect(doneData.reason).toBe('completed')
    }
  })
})