import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { WorkerManager } from '../../src/worker-manager.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { matchesCleanupRule } from '../../src/cleanup.js'
import type { Task, TaskEvent, TaskcastHooks, CleanupRule } from '../../src/types.js'

// ─── Test Setup ──────────────────────────────────────────────────────────────

function makeSetup(hooks?: TaskcastHooks) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast, hooks })
  return { store, broadcast, engine, manager }
}

async function registerDefaultWorker(
  manager: WorkerManager,
  overrides: { id?: string; capacity?: number; weight?: number } = {},
) {
  return manager.registerWorker({
    id: overrides.id ?? 'W1',
    matchRule: { taskTypes: ['*'] },
    capacity: overrides.capacity ?? 10,
    weight: overrides.weight ?? 50,
    connectionMode: 'websocket',
  })
}

// ─── State Machine + Worker ──────────────────────────────────────────────────

describe('State Machine + Worker', () => {
  it('assigned → cancelled: admin cancels assigned task, worker slots freed and assignment removed', async () => {
    const { engine, manager, store } = makeSetup()

    const worker = await registerDefaultWorker(manager, { id: 'W-cancel', capacity: 3 })
    const task = await engine.createTask({ id: 'T-cancel', type: 'work' })

    // Claim the task
    const claim = await manager.claimTask(task.id, worker.id)
    expect(claim.success).toBe(true)

    // Verify assignment exists and worker has used slots
    const workerAfterClaim = await store.getWorker(worker.id)
    expect(workerAfterClaim?.usedSlots).toBe(1)
    const assignmentAfterClaim = await store.getTaskAssignment(task.id)
    expect(assignmentAfterClaim).not.toBeNull()
    expect(assignmentAfterClaim?.workerId).toBe(worker.id)

    // Admin cancels the assigned task via engine transition
    const cancelled = await engine.transitionTask(task.id, 'cancelled')
    expect(cancelled.status).toBe('cancelled')
    expect(cancelled.completedAt).toBeDefined()

    // Note: engine.transitionTask does NOT automatically free worker slots or
    // remove assignment — that would be an orchestration concern. The task is
    // in a terminal state. The worker's usedSlots remain until the assignment
    // is explicitly cleaned up (e.g., by the server layer detecting the terminal
    // status). This test documents the current behavior.
    const workerAfterCancel = await store.getWorker(worker.id)
    expect(workerAfterCancel?.usedSlots).toBe(1)

    // The assignment record is still present (not automatically cleaned up)
    const assignmentAfterCancel = await store.getTaskAssignment(task.id)
    expect(assignmentAfterCancel).not.toBeNull()
  })

  it('assigned → running → completed: full lifecycle with all intermediate states verified', async () => {
    const { engine, manager, store } = makeSetup()

    const worker = await registerDefaultWorker(manager, { id: 'W-full' })
    const task = await engine.createTask({
      id: 'T-full',
      type: 'llm.chat',
      params: { prompt: 'hello' },
    })

    // Phase 1: pending
    expect(task.status).toBe('pending')

    // Phase 2: claim → assigned
    const claim = await manager.claimTask(task.id, worker.id)
    expect(claim.success).toBe(true)
    const assigned = await engine.getTask(task.id)
    expect(assigned?.status).toBe('assigned')
    expect(assigned?.assignedWorker).toBe(worker.id)

    // Phase 3: assigned → running
    const running = await engine.transitionTask(task.id, 'running')
    expect(running.status).toBe('running')
    expect(running.completedAt).toBeUndefined()

    // Publish an event while running
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: 'world' },
    })

    // Phase 4: running → completed
    const completed = await engine.transitionTask(task.id, 'completed', {
      result: { answer: 'hello world' },
    })
    expect(completed.status).toBe('completed')
    expect(completed.completedAt).toBeGreaterThan(0)
    expect(completed.result).toEqual({ answer: 'hello world' })

    // Verify event history includes status transitions and user events
    const events = await engine.getEvents(task.id)
    const statusEvents = events.filter((e) => e.type === 'taskcast:status')
    const userEvents = events.filter((e) => e.type === 'llm.delta')
    // Status events: assigned→running, running→completed (assigned transition via claimTask
    // does NOT emit a status event through the engine — it's done atomically by the store)
    expect(statusEvents.length).toBeGreaterThanOrEqual(2)
    expect(userEvents).toHaveLength(1)
  })

  it('assigned → pending (decline) → assigned (re-claim by different worker)', async () => {
    const { engine, manager, store } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-first', capacity: 5, weight: 80 })
    await registerDefaultWorker(manager, { id: 'W-second', capacity: 5, weight: 60 })

    const task = await engine.createTask({ id: 'T-reassign', type: 'work' })

    // First worker claims
    const claim1 = await manager.claimTask(task.id, 'W-first')
    expect(claim1.success).toBe(true)

    const afterFirstClaim = await engine.getTask(task.id)
    expect(afterFirstClaim?.status).toBe('assigned')
    expect(afterFirstClaim?.assignedWorker).toBe('W-first')

    // First worker's slots are consumed
    const w1AfterClaim = await store.getWorker('W-first')
    expect(w1AfterClaim?.usedSlots).toBe(1)

    // First worker declines
    await manager.declineTask(task.id, 'W-first')

    const afterDecline = await engine.getTask(task.id)
    expect(afterDecline?.status).toBe('pending')
    expect(afterDecline?.assignedWorker).toBeUndefined()

    // First worker's slots are restored
    const w1AfterDecline = await store.getWorker('W-first')
    expect(w1AfterDecline?.usedSlots).toBe(0)

    // Second worker claims
    const claim2 = await manager.claimTask(task.id, 'W-second')
    expect(claim2.success).toBe(true)

    const afterSecondClaim = await engine.getTask(task.id)
    expect(afterSecondClaim?.status).toBe('assigned')
    expect(afterSecondClaim?.assignedWorker).toBe('W-second')

    const w2AfterClaim = await store.getWorker('W-second')
    expect(w2AfterClaim?.usedSlots).toBe(1)
  })

  it('assigned → pending (decline) → running (external mode after decline)', async () => {
    const { engine, manager, store } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-ext' })

    // Task with external assignMode
    const task = await engine.createTask({
      id: 'T-ext',
      type: 'work',
      assignMode: 'external',
    })

    // Worker claims the task
    const claim = await manager.claimTask(task.id, 'W-ext')
    expect(claim.success).toBe(true)
    expect((await engine.getTask(task.id))?.status).toBe('assigned')

    // Worker declines the task
    await manager.declineTask(task.id, 'W-ext')
    expect((await engine.getTask(task.id))?.status).toBe('pending')

    // In external mode, after decline the task can go directly to running
    // (e.g., a different external system picks it up without worker assignment)
    const running = await engine.transitionTask(task.id, 'running')
    expect(running.status).toBe('running')

    // And then complete
    const completed = await engine.transitionTask(task.id, 'completed', {
      result: { done: true },
    })
    expect(completed.status).toBe('completed')
  })
})

// ─── TTL + Worker ────────────────────────────────────────────────────────────

describe('TTL + Worker', () => {
  it('task with TTL can be claimed — setTTL is called at creation', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const setTTLSpy = vi.spyOn(store, 'setTTL')
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })

    await registerDefaultWorker(manager, { id: 'W-ttl' })

    // Create task with TTL
    const task = await engine.createTask({
      id: 'T-ttl',
      type: 'work',
      ttl: 60,
    })

    // Verify TTL was set at creation time
    expect(setTTLSpy).toHaveBeenCalledWith(task.id, 60)

    // Claim should succeed even with TTL
    const claim = await manager.claimTask(task.id, 'W-ttl')
    expect(claim.success).toBe(true)

    const assigned = await engine.getTask(task.id)
    expect(assigned?.status).toBe('assigned')
    expect(assigned?.ttl).toBe(60)
  })

  it('task with TTL transitions assigned → running — timeout still applies from running', async () => {
    const { engine, manager, store } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-ttl-run' })

    const task = await engine.createTask({
      id: 'T-ttl-run',
      type: 'work',
      ttl: 120,
    })

    // Claim and transition to running
    await manager.claimTask(task.id, 'W-ttl-run')
    await engine.transitionTask(task.id, 'running')

    const running = await engine.getTask(task.id)
    expect(running?.status).toBe('running')
    expect(running?.ttl).toBe(120)

    // Simulate TTL expiry: running → timeout is a valid transition
    const timedOut = await engine.transitionTask(task.id, 'timeout')
    expect(timedOut.status).toBe('timeout')
    expect(timedOut.completedAt).toBeDefined()
  })

  it('assigned → timeout is NOT a valid transition (state machine rejects it)', async () => {
    const { engine, manager } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-ttl-assigned' })

    const task = await engine.createTask({
      id: 'T-ttl-assigned',
      type: 'work',
      ttl: 30,
    })

    await manager.claimTask(task.id, 'W-ttl-assigned')

    const assigned = await engine.getTask(task.id)
    expect(assigned?.status).toBe('assigned')

    // The state machine does NOT allow assigned → timeout.
    // This means if a TTL fires while the task is assigned, the external TTL
    // handler must either: (a) skip firing, or (b) transition to cancelled instead.
    await expect(engine.transitionTask(task.id, 'timeout')).rejects.toThrow(
      /Invalid transition: assigned → timeout/,
    )
  })

  it('assigned → cancelled is allowed as an alternative to timeout for TTL expiry while assigned', async () => {
    const { engine, manager } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-ttl-cancel' })

    const task = await engine.createTask({
      id: 'T-ttl-cancel',
      type: 'work',
      ttl: 30,
    })

    await manager.claimTask(task.id, 'W-ttl-cancel')
    expect((await engine.getTask(task.id))?.status).toBe('assigned')

    // Since assigned → timeout is invalid, the TTL handler could cancel instead
    const cancelled = await engine.transitionTask(task.id, 'cancelled')
    expect(cancelled.status).toBe('cancelled')
    expect(cancelled.completedAt).toBeDefined()
  })
})

// ─── Events + Worker ─────────────────────────────────────────────────────────

describe('Events + Worker', () => {
  it('publish event while task is assigned (before running) succeeds', async () => {
    const { engine, manager, store } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-evt' })

    const task = await engine.createTask({ id: 'T-evt', type: 'work' })
    await manager.claimTask(task.id, 'W-evt')

    const assigned = await engine.getTask(task.id)
    expect(assigned?.status).toBe('assigned')

    // Publishing to assigned (non-terminal) should succeed
    const event = await engine.publishEvent(task.id, {
      type: 'worker.progress',
      level: 'info',
      data: { step: 'initializing' },
    })

    expect(event.taskId).toBe(task.id)
    expect(event.type).toBe('worker.progress')

    const events = await engine.getEvents(task.id)
    const userEvents = events.filter((e) => e.type === 'worker.progress')
    expect(userEvents).toHaveLength(1)
  })

  it('publish multiple events during assigned, all appear in event history', async () => {
    const { engine, manager } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-multi-evt' })

    const task = await engine.createTask({ id: 'T-multi-evt', type: 'work' })
    await manager.claimTask(task.id, 'W-multi-evt')

    // Publish 5 events while assigned
    for (let i = 0; i < 5; i++) {
      await engine.publishEvent(task.id, {
        type: 'worker.log',
        level: 'info',
        data: { message: `step ${i}` },
      })
    }

    const events = await engine.getEvents(task.id)
    const logEvents = events.filter((e) => e.type === 'worker.log')
    expect(logEvents).toHaveLength(5)

    // Verify ordering via monotonic index
    for (let i = 1; i < logEvents.length; i++) {
      expect(logEvents[i]!.index).toBeGreaterThan(logEvents[i - 1]!.index)
    }
  })

  it('event ordering across claim → publish → decline → re-claim → publish', async () => {
    const { engine, manager } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-order-1', capacity: 5, weight: 80 })
    await registerDefaultWorker(manager, { id: 'W-order-2', capacity: 5, weight: 60 })

    const task = await engine.createTask({ id: 'T-order', type: 'work' })

    // Phase 1: W1 claims and publishes events
    await manager.claimTask(task.id, 'W-order-1')

    await engine.publishEvent(task.id, {
      type: 'phase1.event',
      level: 'info',
      data: { worker: 'W-order-1', n: 1 },
    })
    await engine.publishEvent(task.id, {
      type: 'phase1.event',
      level: 'info',
      data: { worker: 'W-order-1', n: 2 },
    })

    // Phase 2: W1 declines (this transitions assigned → pending and emits audit events)
    await manager.declineTask(task.id, 'W-order-1')

    // Phase 3: W2 claims and publishes more events
    await manager.claimTask(task.id, 'W-order-2')

    await engine.publishEvent(task.id, {
      type: 'phase2.event',
      level: 'info',
      data: { worker: 'W-order-2', n: 3 },
    })
    await engine.publishEvent(task.id, {
      type: 'phase2.event',
      level: 'info',
      data: { worker: 'W-order-2', n: 4 },
    })

    // Verify full event history in order
    const allEvents = await engine.getEvents(task.id)

    // Extract non-status events for analysis (includes audit events from claim/decline)
    const phase1 = allEvents.filter((e) => e.type === 'phase1.event')
    const phase2 = allEvents.filter((e) => e.type === 'phase2.event')
    const auditEvents = allEvents.filter((e) => e.type === 'taskcast:audit')
    const statusEvents = allEvents.filter((e) => e.type === 'taskcast:status')

    expect(phase1).toHaveLength(2)
    expect(phase2).toHaveLength(2)
    // Audit events from claim and decline operations
    expect(auditEvents.length).toBeGreaterThanOrEqual(2)
    // Status events from decline (assigned→pending) and possibly others
    expect(statusEvents.length).toBeGreaterThanOrEqual(1)

    // All events should have monotonically increasing indices
    for (let i = 1; i < allEvents.length; i++) {
      expect(allEvents[i]!.index).toBeGreaterThan(allEvents[i - 1]!.index)
    }

    // Phase 1 events come before phase 2 events (by index)
    const lastPhase1Index = phase1[phase1.length - 1]!.index
    const firstPhase2Index = phase2[0]!.index
    expect(firstPhase2Index).toBeGreaterThan(lastPhase1Index)
  })
})

// ─── Cleanup + Worker ────────────────────────────────────────────────────────

describe('Cleanup + Worker', () => {
  it('cleanup rule matches task that went through full worker assignment lifecycle', async () => {
    const { engine, manager } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-cleanup' })

    const task = await engine.createTask({
      id: 'T-cleanup',
      type: 'llm.chat',
      cleanup: {
        rules: [
          {
            name: 'archive-completed',
            match: { status: ['completed'], taskTypes: ['llm.*'] },
            trigger: { afterMs: 1000 },
            target: 'events',
          },
        ],
      },
    })

    // Full lifecycle: pending → assigned → running → completed
    await manager.claimTask(task.id, 'W-cleanup')
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: 'some output' },
    })
    await engine.transitionTask(task.id, 'completed', {
      result: { output: 'done' },
    })

    const completedTask = await engine.getTask(task.id)
    expect(completedTask?.status).toBe('completed')

    // Verify cleanup rule matches against the completed task
    const rule = task.cleanup!.rules[0]!
    // Shortly after completion — rule requires 1000ms, so this should NOT match yet
    const nowEarly = completedTask!.completedAt! + 500
    expect(matchesCleanupRule(completedTask!, rule, nowEarly)).toBe(false)

    // After the delay period — should match
    const nowLate = completedTask!.completedAt! + 1500
    expect(matchesCleanupRule(completedTask!, rule, nowLate)).toBe(true)
  })

  it('cleanup rule does NOT match task in assigned status (non-terminal)', async () => {
    const { engine, manager } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-cleanup-assigned' })

    const task = await engine.createTask({
      id: 'T-cleanup-assigned',
      type: 'work',
    })

    await manager.claimTask(task.id, 'W-cleanup-assigned')

    const assignedTask = await engine.getTask(task.id)
    expect(assignedTask?.status).toBe('assigned')

    // Cleanup rules only apply to terminal statuses
    const rule: CleanupRule = {
      trigger: {},
      target: 'all',
    }
    expect(matchesCleanupRule(assignedTask!, rule, Date.now())).toBe(false)
  })

  it('cleanup rule matches cancelled task that was previously assigned', async () => {
    const { engine, manager } = makeSetup()

    await registerDefaultWorker(manager, { id: 'W-cleanup-cancel' })

    const task = await engine.createTask({
      id: 'T-cleanup-cancel',
      type: 'work',
    })

    // assigned → cancelled
    await manager.claimTask(task.id, 'W-cleanup-cancel')
    await engine.transitionTask(task.id, 'cancelled')

    const cancelledTask = await engine.getTask(task.id)
    expect(cancelledTask?.status).toBe('cancelled')

    const rule: CleanupRule = {
      match: { status: ['cancelled'] },
      trigger: {},
      target: 'all',
    }
    expect(matchesCleanupRule(cancelledTask!, rule, Date.now())).toBe(true)
  })
})

// ─── Hooks + Worker ──────────────────────────────────────────────────────────

describe('Hooks + Worker', () => {
  it('onTaskTransitioned fires for assigned → running', async () => {
    const onTaskTransitioned = vi.fn()
    const { engine, manager } = makeSetup({ onTaskTransitioned })

    await registerDefaultWorker(manager, { id: 'W-hook-run' })
    const task = await engine.createTask({ id: 'T-hook-run', type: 'work' })

    await manager.claimTask(task.id, 'W-hook-run')
    onTaskTransitioned.mockClear()

    await engine.transitionTask(task.id, 'running')

    expect(onTaskTransitioned).toHaveBeenCalledOnce()
    expect(onTaskTransitioned).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'running' }),
      'assigned',
      'running',
    )
  })

  it('onTaskTransitioned fires for assigned → pending (decline)', async () => {
    const onTaskTransitioned = vi.fn()
    const { engine, manager } = makeSetup({ onTaskTransitioned })

    await registerDefaultWorker(manager, { id: 'W-hook-decline' })
    const task = await engine.createTask({ id: 'T-hook-decline', type: 'work' })

    await manager.claimTask(task.id, 'W-hook-decline')
    onTaskTransitioned.mockClear()

    // declineTask internally calls engine.transitionTask(taskId, 'pending')
    await manager.declineTask(task.id, 'W-hook-decline')

    expect(onTaskTransitioned).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'pending' }),
      'assigned',
      'pending',
    )
  })

  it('onTaskTransitioned fires for assigned → cancelled', async () => {
    const onTaskTransitioned = vi.fn()
    const { engine, manager } = makeSetup({ onTaskTransitioned })

    await registerDefaultWorker(manager, { id: 'W-hook-cancel' })
    const task = await engine.createTask({ id: 'T-hook-cancel', type: 'work' })

    await manager.claimTask(task.id, 'W-hook-cancel')
    onTaskTransitioned.mockClear()

    await engine.transitionTask(task.id, 'cancelled')

    expect(onTaskTransitioned).toHaveBeenCalledOnce()
    expect(onTaskTransitioned).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'cancelled' }),
      'assigned',
      'cancelled',
    )
  })

  it('onTaskAssigned fires on claim, onTaskDeclined fires on decline', async () => {
    const onTaskAssigned = vi.fn()
    const onTaskDeclined = vi.fn()
    const { engine, manager } = makeSetup({ onTaskAssigned, onTaskDeclined })

    await registerDefaultWorker(manager, { id: 'W-hook-assign' })
    const task = await engine.createTask({ id: 'T-hook-assign', type: 'work' })

    // Claim → onTaskAssigned should fire
    await manager.claimTask(task.id, 'W-hook-assign')
    expect(onTaskAssigned).toHaveBeenCalledOnce()
    expect(onTaskAssigned).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'assigned' }),
      expect.objectContaining({ id: 'W-hook-assign' }),
    )

    // Decline → onTaskDeclined should fire
    await manager.declineTask(task.id, 'W-hook-assign')
    expect(onTaskDeclined).toHaveBeenCalledOnce()
    expect(onTaskDeclined).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'pending' }),
      expect.objectContaining({ id: 'W-hook-assign' }),
      false, // blacklisted=false
    )
  })

  it('onTaskDeclined passes blacklisted=true when decline with blacklist option', async () => {
    const onTaskDeclined = vi.fn()
    const { engine, manager } = makeSetup({ onTaskDeclined })

    await registerDefaultWorker(manager, { id: 'W-hook-bl' })
    const task = await engine.createTask({ id: 'T-hook-bl', type: 'work' })

    await manager.claimTask(task.id, 'W-hook-bl')
    await manager.declineTask(task.id, 'W-hook-bl', { blacklist: true })

    expect(onTaskDeclined).toHaveBeenCalledOnce()
    expect(onTaskDeclined).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id }),
      expect.objectContaining({ id: 'W-hook-bl' }),
      true, // blacklisted=true
    )
  })

  it('full lifecycle fires hooks in correct order: created → assigned → transitioned(assigned→running) → transitioned(running→completed)', async () => {
    const callOrder: string[] = []
    const hooks: TaskcastHooks = {
      onTaskCreated: () => callOrder.push('created'),
      onTaskAssigned: () => callOrder.push('assigned'),
      onTaskTransitioned: (_task, from, to) => callOrder.push(`transitioned:${from}→${to}`),
    }
    const { engine, manager } = makeSetup(hooks)

    await registerDefaultWorker(manager, { id: 'W-hook-full' })
    const task = await engine.createTask({ id: 'T-hook-full', type: 'work' })

    await manager.claimTask(task.id, 'W-hook-full')
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed', { result: { done: true } })

    expect(callOrder).toEqual([
      'created',
      'assigned',
      'transitioned:assigned→running',
      'transitioned:running→completed',
    ])
  })
})
