import { describe, it, expect } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'
import type { AuthContext } from '../src/auth.js'

function makeApp(opts?: { withWorkerManager?: boolean }) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const withWM = opts?.withWorkerManager ?? true
  const manager = withWM
    ? new WorkerManager({ engine, shortTermStore: store, broadcast })
    : undefined
  const app = createTaskcastApp({ engine, workerManager: manager, auth: { mode: 'none' } })
  return { app, engine, manager, store }
}

describe('Server — worker capacity release on terminal transition', () => {
  it('PATCH to completed releases worker capacity', async () => {
    const { app, engine, manager, store } = makeApp()

    // Register worker
    const worker = await manager!.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Create task and claim it
    const task = await engine.createTask({ type: 'test', cost: 2 })
    await manager!.claimTask(task.id, worker.id)

    const workerBusy = await store.getWorker(worker.id)
    expect(workerBusy!.usedSlots).toBe(2)

    // Transition to running via PATCH
    const runRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)

    // Transition to completed via PATCH — should trigger releaseTask
    const completeRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(completeRes.status).toBe(200)

    // Allow async releaseTask to complete
    await Promise.resolve()
    await Promise.resolve()

    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBe(0)
    expect(workerAfter!.status).toBe('idle')
  })

  it('PATCH to failed releases capacity', async () => {
    const { app, engine, manager, store } = makeApp()

    const worker = await manager!.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager!.claimTask(task.id, worker.id)

    // Transition to running, then failed
    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    const failRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'failed', error: { message: 'boom' } }),
    })
    expect(failRes.status).toBe(200)

    await Promise.resolve()
    await Promise.resolve()

    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBe(0)
    expect(workerAfter!.status).toBe('idle')
  })

  it('PATCH to cancelled releases capacity (assigned -> cancelled)', async () => {
    const { app, engine, manager, store } = makeApp()

    const worker = await manager!.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const task = await engine.createTask({ type: 'test', cost: 3 })
    await manager!.claimTask(task.id, worker.id)

    const workerBefore = await store.getWorker(worker.id)
    expect(workerBefore!.usedSlots).toBe(3)

    // Assigned -> cancelled is a valid transition
    const cancelRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'cancelled' }),
    })
    expect(cancelRes.status).toBe(200)

    await Promise.resolve()
    await Promise.resolve()

    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBe(0)
    expect(workerAfter!.status).toBe('idle')
  })

  it('PATCH to running does NOT release capacity', async () => {
    const { app, engine, manager, store } = makeApp()

    const worker = await manager!.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const task = await engine.createTask({ type: 'test', cost: 2 })
    await manager!.claimTask(task.id, worker.id)

    const workerBefore = await store.getWorker(worker.id)
    expect(workerBefore!.usedSlots).toBe(2)

    // Transition to running (non-terminal) should NOT release
    const runRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)

    await Promise.resolve()
    await Promise.resolve()

    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBe(2)

    // Assignment should still exist
    const assignment = await store.getTaskAssignment(task.id)
    expect(assignment).not.toBeNull()
  })

  it('server without workerManager — no error on terminal transition', async () => {
    const { app, engine } = makeApp({ withWorkerManager: false })

    const task = await engine.createTask({ type: 'test' })

    const runRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)

    // Completing a task without a workerManager should not error
    const completeRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(completeRes.status).toBe(200)

    const body = await completeRes.json()
    expect(body.status).toBe('completed')
  })

  it('full flow: claim -> run -> complete -> worker becomes idle -> can accept new task again', async () => {
    const { app, engine, manager, store } = makeApp()

    const worker = await manager!.registerWorker({
      matchRule: {},
      capacity: 1,
      connectionMode: 'pull',
    })

    // First task: claim, run, complete
    const task1 = await engine.createTask({ type: 'test', cost: 1 })
    await manager!.claimTask(task1.id, worker.id)

    // Worker should be busy (capacity 1, usedSlots 1)
    const workerBusy = await store.getWorker(worker.id)
    expect(workerBusy!.status).toBe('busy')
    expect(workerBusy!.usedSlots).toBe(1)

    // Transition to running
    await app.request(`/tasks/${task1.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Transition to completed
    await app.request(`/tasks/${task1.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })

    await Promise.resolve()
    await Promise.resolve()

    // Worker should be idle and ready for new work
    const workerIdle = await store.getWorker(worker.id)
    expect(workerIdle!.status).toBe('idle')
    expect(workerIdle!.usedSlots).toBe(0)

    // Second task: should be claimable now
    const task2 = await engine.createTask({ type: 'test', cost: 1 })
    const claimResult = await manager!.claimTask(task2.id, worker.id)
    expect(claimResult.success).toBe(true)

    const workerBusy2 = await store.getWorker(worker.id)
    expect(workerBusy2!.status).toBe('busy')
    expect(workerBusy2!.usedSlots).toBe(1)
  })
})
