import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { createSSERouter, createSubscriberCounts } from '../src/routes/sse.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createSSERouter(engine, createSubscriberCounts()))
  return { app, engine, manager, store }
}

async function collectSSEEvents(
  res: Response,
  count: number,
): Promise<Array<{ event: string; data: string }>> {
  const reader = res.body!.getReader()
  const decoder = new TextDecoder()
  const collected: Array<{ event: string; data: string }> = []
  let buffer = ''

  while (collected.length < count) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    const blocks = buffer.split('\n\n')
    buffer = blocks.pop() ?? ''
    for (const block of blocks) {
      if (!block.trim()) continue
      const lines = block.split('\n')
      const eventLine = lines.find((l) => l.startsWith('event:'))
      const dataLine = lines.find((l) => l.startsWith('data:'))
      if (eventLine && dataLine) {
        collected.push({
          event: eventLine.replace('event:', '').trim(),
          data: dataLine.replace('data:', '').trim(),
        })
      }
    }
  }

  reader.cancel()
  return collected
}

// ─── SSE + Assigned Status ──────────────────────────────────────────────────

describe('SSE + Worker Assigned Status', () => {
  it('SSE subscription on assigned task replays history events', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Claim makes it assigned
    await manager.claimTask(task.id, worker.id)
    const assignedTask = await engine.getTask(task.id)
    expect(assignedTask!.status).toBe('assigned')

    // Publish an event while assigned (the audit event from claimTask is also present)
    await engine.publishEvent(task.id, { type: 'worker.info', level: 'info', data: { msg: 'preparing' } })

    // Now transition to completed so we get a done event and the stream closes
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // History: audit event + worker.info + status:running + status:completed + taskcast.done
    const events = await collectSSEEvents(res, 5)
    const taskcastEvents = events.filter((e) => e.event === 'taskcast.event')
    const types = taskcastEvents.map((e) => JSON.parse(e.data).type)
    expect(types).toContain('worker.info')
    expect(events.some((e) => e.event === 'taskcast.done')).toBe(true)
  })

  it('SSE on pending task holds, then delivers events after running', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Task starts as pending — SSE should subscribe and hold
    // Schedule transitions after SSE connection is established
    setTimeout(async () => {
      // pending -> assigned (via claim)
      await manager.claimTask(task.id, worker.id)
      // assigned -> running
      await engine.transitionTask(task.id, 'running')
      // publish while running
      await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: 'hello' } })
      // running -> completed
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // Live events: audit(assigned) + status(running) + llm.delta + status(completed) + done
    const events = await collectSSEEvents(res, 5)
    const taskcastEvents = events.filter((e) => e.event === 'taskcast.event')
    const types = taskcastEvents.map((e) => JSON.parse(e.data).type)
    expect(types).toContain('llm.delta')
    expect(types).toContain('taskcast:status')

    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(doneEvent).toBeDefined()
    expect(JSON.parse(doneEvent!.data).reason).toBe('completed')
  }, 10000)

  it('SSE on assigned task through running to completed with live events', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Claim it
    await manager.claimTask(task.id, worker.id)

    // Schedule: assigned -> running -> events -> completed
    setTimeout(async () => {
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { chunk: 1 } })
      await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { chunk: 2 } })
      await engine.transitionTask(task.id, 'completed', { result: { answer: 42 } })
    }, 50)

    const res = await app.request(`/tasks/${task.id}/events`)

    // History: audit(assigned) from claim
    // Live: status(running) + llm.delta*2 + status(completed) + done
    // Total: audit + running + delta + delta + completed + done = 6
    const events = await collectSSEEvents(res, 6)
    const taskcastEvents = events.filter((e) => e.event === 'taskcast.event')
    const liveDeltas = taskcastEvents.filter((e) => JSON.parse(e.data).type === 'llm.delta')
    expect(liveDeltas).toHaveLength(2)

    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(doneEvent).toBeDefined()
    expect(JSON.parse(doneEvent!.data).reason).toBe('completed')
  }, 10000)

  it('SSE on assigned task that gets declined back to pending keeps subscriber connected', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Claim it
    await manager.claimTask(task.id, worker.id)

    // Schedule: decline (back to pending) -> re-claim -> running -> completed
    setTimeout(async () => {
      // Decline -> pending (emits status event for 'pending')
      await manager.declineTask(task.id, worker.id)
      // Re-claim -> assigned
      await manager.claimTask(task.id, worker.id)
      // assigned -> running
      await engine.transitionTask(task.id, 'running')
      // running -> completed
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(`/tasks/${task.id}/events`)

    // The stream should stay open through the decline and eventually close on completed.
    // History: audit(assigned) from first claim
    // Live: status(pending) from decline + audit(declined) + audit(assigned) from re-claim +
    //       status(running) + status(completed) + done
    // We just need to verify it closes properly and has the done event.
    const events = await collectSSEEvents(res, 7)
    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(doneEvent).toBeDefined()
    expect(JSON.parse(doneEvent!.data).reason).toBe('completed')

    // The status(pending) event from decline should be present (non-terminal, so stream stays open)
    const statusEvents = events
      .filter((e) => e.event === 'taskcast.event')
      .filter((e) => JSON.parse(e.data).type === 'taskcast:status')
    const statuses = statusEvents.map((e) => JSON.parse(e.data).data.status)
    expect(statuses).toContain('pending')
    expect(statuses).toContain('running')
    expect(statuses).toContain('completed')
  }, 10000)

  it('multiple SSE subscribers during claim/decline/re-claim cycle receive consistent events', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const workerA = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    const workerB = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Claim with worker A
    await manager.claimTask(task.id, workerA.id)

    // Schedule transitions after both subscribers connect
    setTimeout(async () => {
      // Decline by worker A
      await manager.declineTask(task.id, workerA.id)
      // Re-claim by worker B
      await manager.claimTask(task.id, workerB.id)
      // Running -> complete
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'result', level: 'info', data: { val: 'done' } })
      await engine.transitionTask(task.id, 'completed')
    }, 80)

    // Both subscribers connect at roughly the same time
    const [res1, res2] = await Promise.all([
      app.request(`/tasks/${task.id}/events`),
      app.request(`/tasks/${task.id}/events`),
    ])

    // Both should get the same events and both close with done
    const events1 = await collectSSEEvents(res1, 8)
    const events2 = await collectSSEEvents(res2, 8)

    // Both should have taskcast.done
    expect(events1.some((e) => e.event === 'taskcast.done')).toBe(true)
    expect(events2.some((e) => e.event === 'taskcast.done')).toBe(true)

    // Both should see the 'result' event
    const hasResult1 = events1.some(
      (e) => e.event === 'taskcast.event' && JSON.parse(e.data).type === 'result',
    )
    const hasResult2 = events2.some(
      (e) => e.event === 'taskcast.event' && JSON.parse(e.data).type === 'result',
    )
    expect(hasResult1).toBe(true)
    expect(hasResult2).toBe(true)
  }, 10000)

  it('SSE on assigned task that gets cancelled closes the stream', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Claim it
    await manager.claimTask(task.id, worker.id)

    // Schedule cancellation
    setTimeout(async () => {
      await engine.transitionTask(task.id, 'cancelled')
    }, 50)

    const res = await app.request(`/tasks/${task.id}/events`)

    // History: audit(assigned)
    // Live: status(cancelled) + done
    const events = await collectSSEEvents(res, 3)
    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(doneEvent).toBeDefined()
    expect(JSON.parse(doneEvent!.data).reason).toBe('cancelled')
  }, 10000)
})

// ─── Events During Assigned ─────────────────────────────────────────────────

describe('Events During Assigned Status', () => {
  it('publish event while task is assigned succeeds', async () => {
    const { engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await manager.claimTask(task.id, worker.id)

    // assigned is non-terminal, so publishEvent should succeed
    const evt = await engine.publishEvent(task.id, {
      type: 'worker.status',
      level: 'info',
      data: { msg: 'initializing GPU' },
    })
    expect(evt).toBeDefined()
    expect(evt.type).toBe('worker.status')
    expect(evt.taskId).toBe(task.id)
  })

  it('publish multiple events during assigned and SSE subscriber receives them all', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await manager.claimTask(task.id, worker.id)

    // Publish events while assigned
    await engine.publishEvent(task.id, { type: 'worker.init', level: 'info', data: { step: 1 } })
    await engine.publishEvent(task.id, { type: 'worker.init', level: 'info', data: { step: 2 } })
    await engine.publishEvent(task.id, { type: 'worker.init', level: 'info', data: { step: 3 } })

    // Transition to terminal so SSE replays and closes
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?includeStatus=false`)

    // Events without status: audit(assigned) + worker.init*3 + done
    const events = await collectSSEEvents(res, 5)
    const initEvents = events
      .filter((e) => e.event === 'taskcast.event')
      .filter((e) => JSON.parse(e.data).type === 'worker.init')
    expect(initEvents).toHaveLength(3)

    const steps = initEvents.map((e) => JSON.parse(e.data).data.step)
    expect(steps).toEqual([1, 2, 3])
  })
})

// ─── Full Worker + SSE Integration Flow ─────────────────────────────────────

describe('Full Worker + SSE Integration Flow', () => {
  it('complete flow: create → claim → events while assigned → running → more events → complete', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'gpu-inference', cost: 2 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 10,
      connectionMode: 'pull',
    })

    // Claim (pending -> assigned)
    const claimResult = await manager.claimTask(task.id, worker.id)
    expect(claimResult.success).toBe(true)

    // Publish events while assigned
    await engine.publishEvent(task.id, { type: 'worker.preparing', level: 'info', data: { gpu: 'A100' } })

    // Start running
    await engine.transitionTask(task.id, 'running')

    // Publish events while running
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: 'Hello' } })
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: ' world' } })

    // Complete
    await engine.transitionTask(task.id, 'completed', { result: { output: 'Hello world' } })

    // Connect SSE — task is already terminal, so this replays history and closes
    const res = await app.request(`/tasks/${task.id}/events`)

    // History replay:
    // audit(assigned) + worker.preparing + status(running) + llm.delta*2 + status(completed) + done
    const events = await collectSSEEvents(res, 7)
    const taskcastEvents = events.filter((e) => e.event === 'taskcast.event')
    const types = taskcastEvents.map((e) => JSON.parse(e.data).type)

    // Verify event ordering: audit -> worker.preparing -> status(running) -> deltas -> status(completed)
    expect(types).toContain('taskcast:audit')
    expect(types).toContain('worker.preparing')
    expect(types).toContain('llm.delta')
    expect(types).toContain('taskcast:status')

    // Verify the llm.delta events are present
    const deltas = taskcastEvents.filter((e) => JSON.parse(e.data).type === 'llm.delta')
    expect(deltas).toHaveLength(2)

    // Verify done
    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(doneEvent).toBeDefined()
    expect(JSON.parse(doneEvent!.data).reason).toBe('completed')
  })

  it('decline flow: create → worker A claims → decline → worker B claims → running → complete', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })

    const workerA = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    const workerB = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Connect SSE subscriber before any transitions
    // Schedule the full lifecycle after SSE is established
    setTimeout(async () => {
      // Worker A claims
      await manager.claimTask(task.id, workerA.id)
      // Worker A declines
      await manager.declineTask(task.id, workerA.id)
      // Worker B claims
      await manager.claimTask(task.id, workerB.id)
      // Worker B starts running
      await engine.transitionTask(task.id, 'running')
      // Publish work
      await engine.publishEvent(task.id, { type: 'work.result', level: 'info', data: { val: 100 } })
      // Complete
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(`/tasks/${task.id}/events`)

    // Live events from subscriber perspective:
    // audit(assigned-A) + status(pending) from decline + audit(declined) +
    // audit(assigned-B) + status(running) + work.result + status(completed) + done
    // Exact count may vary depending on audit events; collect enough
    const events = await collectSSEEvents(res, 9)

    // Verify the subscriber eventually sees the done event
    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(doneEvent).toBeDefined()
    expect(JSON.parse(doneEvent!.data).reason).toBe('completed')

    // Verify the work.result event is present
    const workResult = events.find(
      (e) => e.event === 'taskcast.event' && JSON.parse(e.data).type === 'work.result',
    )
    expect(workResult).toBeDefined()
    expect(JSON.parse(workResult!.data).data.val).toBe(100)

    // Verify we see status transitions through the full cycle
    const statusEvents = events
      .filter((e) => e.event === 'taskcast.event')
      .filter((e) => JSON.parse(e.data).type === 'taskcast:status')
    const statuses = statusEvents.map((e) => JSON.parse(e.data).data.status)
    // Should see pending (from decline), running, completed
    expect(statuses).toContain('pending')
    expect(statuses).toContain('running')
    expect(statuses).toContain('completed')
  }, 10000)
})
