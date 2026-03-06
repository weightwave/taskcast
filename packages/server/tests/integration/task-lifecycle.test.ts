import { describe, it, expect } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'

describe('Server integration — task lifecycle', () => {
  it('POST create -> POST events -> PATCH complete -> GET query', async () => {
    const { app } = createTestServer()

    // Create task
    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.chat', params: { model: 'gpt-4' } }),
    })
    expect(createRes.status).toBe(201)
    const task = await createRes.json()
    expect(task.id).toBeTruthy()
    expect(task.status).toBe('pending')

    // Transition to running
    const runRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)

    // Publish events
    const evtRes = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.delta', level: 'info', data: { text: 'hello' } }),
    })
    expect(evtRes.status).toBe(201)

    // Complete with result
    const completeRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed', result: { answer: 42 } }),
    })
    expect(completeRes.status).toBe(200)
    const completed = await completeRes.json()
    expect(completed.status).toBe('completed')
    expect(completed.completedAt).toBeTruthy()
    expect(completed.result).toEqual({ answer: 42 })

    // GET final state
    const getRes = await app.request(`/tasks/${task.id}`)
    expect(getRes.status).toBe(200)
    const fetched = await getRes.json()
    expect(fetched.status).toBe('completed')
    expect(fetched.result).toEqual({ answer: 42 })
  })

  it('batch events -> GET history preserves order', async () => {
    const { app } = createTestServer()

    const task = (await (await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })).json())

    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Batch publish
    const events = Array.from({ length: 5 }, (_, i) => ({
      type: 'chunk', level: 'info', data: { index: i },
    }))
    const batchRes = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(events),
    })
    expect(batchRes.status).toBe(201)

    // GET history
    const historyRes = await app.request(`/tasks/${task.id}/events/history`)
    expect(historyRes.status).toBe(200)
    const history = await historyRes.json()
    // History includes taskcast:status(running) + 5 chunks
    const chunks = history.filter((e: { type: string }) => e.type === 'chunk')
    expect(chunks).toHaveLength(5)
    for (let i = 0; i < 5; i++) {
      expect(chunks[i].data.index).toBe(i)
    }
  })

  it('publish to terminal task returns error', async () => {
    const { app } = createTestServer()

    const task = (await (await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({}),
    })).json())

    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })

    const evtRes = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'late', level: 'info', data: null }),
    })
    expect(evtRes.status).toBeGreaterThanOrEqual(400)
  })

  it('JSON round-trip has camelCase fields', async () => {
    const { app } = createTestServer()

    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', metadata: { userId: 'u1' } }),
    })
    const task = await createRes.json()
    expect(task).toHaveProperty('createdAt')
    expect(task).toHaveProperty('updatedAt')
    expect(task).not.toHaveProperty('created_at')
  })
})
