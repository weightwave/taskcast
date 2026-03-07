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
})
