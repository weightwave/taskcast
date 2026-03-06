import { describe, it, expect } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'

describe('Server integration — worker flow', () => {
  it('register -> claim -> usedSlots increases', async () => {
    const { app, engine, store, workerManager } = createTestServer({ withWorkerManager: true })

    // Register worker
    const worker = await workerManager!.registerWorker({
      matchRule: {}, capacity: 5, connectionMode: 'pull',
    })

    // Create task via HTTP
    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', cost: 2 }),
    })
    const task = await createRes.json()
    const claim = await workerManager!.claimTask(task.id, worker.id)
    expect(claim.success).toBe(true)

    const busyWorker = await store.getWorker(worker.id)
    expect(busyWorker!.usedSlots).toBe(2)
  })

  it('claim on non-existent task returns failure', async () => {
    const { workerManager } = createTestServer({ withWorkerManager: true })

    const worker = await workerManager!.registerWorker({
      matchRule: {}, capacity: 5, connectionMode: 'pull',
    })

    const result = await workerManager!.claimTask('nonexistent-task', worker.id)
    expect(result.success).toBe(false)
  })

  it('concurrent claim race — at most one worker succeeds', async () => {
    const { workerManager, engine } = createTestServer({ withWorkerManager: true })

    const workers = await Promise.all(
      Array.from({ length: 5 }, () =>
        workerManager!.registerWorker({
          matchRule: {}, capacity: 1, connectionMode: 'pull',
        })
      )
    )

    const task = await engine.createTask({ type: 'test', cost: 1 })

    const claims = await Promise.all(
      workers.map(w => workerManager!.claimTask(task.id, w.id))
    )
    const successes = claims.filter(c => c.success)
    expect(successes.length).toBeLessThanOrEqual(1)
  })

  it('worker status becomes busy when at capacity', async () => {
    const { engine, store, workerManager } = createTestServer({ withWorkerManager: true })

    const worker = await workerManager!.registerWorker({
      matchRule: {}, capacity: 2, connectionMode: 'pull',
    })

    const task1 = await engine.createTask({ type: 'test', cost: 1 })
    await workerManager!.claimTask(task1.id, worker.id)

    let w = await store.getWorker(worker.id)
    expect(w!.status).toBe('idle')

    const task2 = await engine.createTask({ type: 'test', cost: 1 })
    await workerManager!.claimTask(task2.id, worker.id)

    w = await store.getWorker(worker.id)
    expect(w!.status).toBe('busy')
    expect(w!.usedSlots).toBe(2)
  })
})
