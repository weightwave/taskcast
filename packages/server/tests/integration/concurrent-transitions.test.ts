import { describe, it, expect } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'

describe('Server integration — concurrent transitions', () => {
  it('sequential double-complete — second attempt fails', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const first = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(first.status).toBe(200)

    const second = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(second.status).toBe(400)
  })

  it('task state is consistent after concurrent race', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Race: some try to complete, some try to fail
    await Promise.all([
      ...Array.from({ length: 5 }, () =>
        app.request(`/tasks/${task.id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'completed' }),
        })
      ),
      ...Array.from({ length: 5 }, () =>
        app.request(`/tasks/${task.id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'failed', error: { message: 'oops' } }),
        })
      ),
    ])

    // Final state should be one of the terminal states
    const getRes = await app.request(`/tasks/${task.id}`)
    const final = await getRes.json()
    expect(['completed', 'failed']).toContain(final.status)
  })

  it('10 concurrent PATCH: exactly 1 succeeds, 9 fail with 400', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Fire 10 concurrent PATCH requests: 5 to 'completed', 5 to 'failed'
    const results = await Promise.all([
      ...Array.from({ length: 5 }, () =>
        app.request(`/tasks/${task.id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'completed', result: { done: true } }),
        })
      ),
      ...Array.from({ length: 5 }, () =>
        app.request(`/tasks/${task.id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'failed', error: { message: 'boom' } }),
        })
      ),
    ])

    const statuses = results.map((r) => r.status)
    const successes = statuses.filter((s) => s === 200)
    const failures = statuses.filter((s) => s === 400)

    // Exactly 1 should succeed, rest should be 400
    expect(successes).toHaveLength(1)
    expect(failures).toHaveLength(9)

    // Task should be in exactly one terminal state
    const getRes = await app.request(`/tasks/${task.id}`)
    const final = await getRes.json()
    expect(['completed', 'failed']).toContain(final.status)

    // Verify the winning response body is consistent
    const winnerIdx = statuses.findIndex((s) => s === 200)
    const winnerBody = await results[winnerIdx]!.json()
    expect(winnerBody.status).toBe(final.status)
  })
})
