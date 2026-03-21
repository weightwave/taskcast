import { describe, it, expect, vi, beforeEach } from 'vitest'
import { TaskEngine, SeqStaleError, SeqDuplicateError, SeqGapError, SeqTimeoutError } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { TaskEvent } from '../../src/types.js'

function makeEngine(opts?: { seqHoldTimeout?: number }) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({
    shortTermStore: store,
    broadcast,
    seqHoldTimeout: opts?.seqHoldTimeout,
  })
  return { engine, store, broadcast }
}

async function createRunningTask(engine: TaskEngine, id?: string) {
  const task = await engine.createTask({ id })
  await engine.transitionTask(task.id, 'running')
  return task
}

// ─── MemoryShortTermStore seq methods ──────────────────────────────────────

describe('MemoryShortTermStore seq ordering', () => {
  let store: MemoryShortTermStore

  beforeEach(() => {
    store = new MemoryShortTermStore()
  })

  it('accepts first event and initializes expectedSeq', async () => {
    const result = await store.processSeq('t1', 'c1', 5, 60)
    expect(result).toEqual({ action: 'accept' })
    expect(await store.getExpectedSeq('t1', 'c1')).toBe(6)
  })

  it('accepts sequential events', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    const r1 = await store.processSeq('t1', 'c1', 1, 60)
    expect(r1).toEqual({ action: 'accept' })
    const r2 = await store.processSeq('t1', 'c1', 2, 60)
    expect(r2).toEqual({ action: 'accept' })
    expect(await store.getExpectedSeq('t1', 'c1')).toBe(3)
  })

  it('rejects stale seq', async () => {
    await store.processSeq('t1', 'c1', 5, 60)
    const result = await store.processSeq('t1', 'c1', 3, 60)
    expect(result).toEqual({ action: 'reject_stale', expected: 6 })
  })

  it('waits for future seq', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    const result = await store.processSeq('t1', 'c1', 3, 60)
    expect(result).toEqual({ action: 'wait' })
  })

  it('rejects duplicate registered slot', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    await store.processSeq('t1', 'c1', 3, 60) // register slot
    const result = await store.processSeq('t1', 'c1', 3, 60) // duplicate
    expect(result).toEqual({ action: 'reject_duplicate' })
  })

  it('returns triggerNext when next slot is registered', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    await store.processSeq('t1', 'c1', 2, 60) // register slot 2
    const result = await store.processSeq('t1', 'c1', 1, 60) // fill gap
    expect(result).toEqual({ action: 'accept', triggerNext: 2 })
  })

  it('rejects duplicate when seq > expected and slot exists', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    await store.processSeq('t1', 'c1', 3, 60) // register slot 3
    const result = await store.processSeq('t1', 'c1', 3, 60)
    expect(result).toEqual({ action: 'reject_duplicate' })
  })

  it('rejects duplicate when seq == expected and slot was registered early', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    await store.processSeq('t1', 'c1', 2, 60) // register slot 2
    const r = await store.processSeq('t1', 'c1', 1, 60) // accept, triggerNext=2
    expect(r).toEqual({ action: 'accept', triggerNext: 2 })
    // expected=2, slot[2] still exists → reject_duplicate
    const r2 = await store.processSeq('t1', 'c1', 2, 60)
    expect(r2).toEqual({ action: 'reject_duplicate' })
  })

  it('does not return triggerNext when next slot is not registered', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    await store.processSeq('t1', 'c1', 5, 60) // register slot 5 (far ahead)
    const result = await store.processSeq('t1', 'c1', 1, 60) // seq=1, expected=1
    expect(result).toEqual({ action: 'accept' }) // no triggerNext since slot[2] doesn't exist
  })

  it('handles multiple independent tasks/clients', async () => {
    await store.processSeq('t1', 'c1', 0, 60)
    await store.processSeq('t1', 'c2', 10, 60)
    await store.processSeq('t2', 'c1', 100, 60)
    expect(await store.getExpectedSeq('t1', 'c1')).toBe(1)
    expect(await store.getExpectedSeq('t1', 'c2')).toBe(11)
    expect(await store.getExpectedSeq('t2', 'c1')).toBe(101)
  })

  describe('advanceAfterEmit', () => {
    it('triggers next slot after emit', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      await store.processSeq('t1', 'c1', 2, 60)
      await store.processSeq('t1', 'c1', 1, 60) // accept + triggerNext=2
      const advance = await store.advanceAfterEmit('t1', 'c1', 2, 60)
      expect(advance).toEqual({})
      expect(await store.getExpectedSeq('t1', 'c1')).toBe(3)
    })

    it('cascades through consecutive registered slots', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      await store.processSeq('t1', 'c1', 2, 60)
      await store.processSeq('t1', 'c1', 3, 60)
      const r = await store.processSeq('t1', 'c1', 1, 60)
      expect(r).toEqual({ action: 'accept', triggerNext: 2 })
      const advance = await store.advanceAfterEmit('t1', 'c1', 2, 60)
      expect(advance).toEqual({ triggerNext: 3 })
    })

    it('returns empty when no state', async () => {
      const result = await store.advanceAfterEmit('t1', 'c1', 0, 60)
      expect(result).toEqual({})
    })

    it('advances expected when equal to completedSeq', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      // expected is 1, advance after emitting 0 should not change it (expected != 0)
      // But advance after a triggered event: expected should be completedSeq, set to completedSeq+1
      // Simulate: expected=5, advance(5) → expected=6
      // We need to get expected to be 5 without advancing:
      // Process 0,1,2,3,4 → expected=5
      for (let i = 1; i <= 4; i++) {
        await store.processSeq('t1', 'c1', i, 60)
      }
      expect(await store.getExpectedSeq('t1', 'c1')).toBe(5)
      const r = await store.advanceAfterEmit('t1', 'c1', 5, 60)
      expect(r).toEqual({})
      expect(await store.getExpectedSeq('t1', 'c1')).toBe(6)
    })
  })

  describe('cancelSlot', () => {
    it('cancels a registered slot', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      await store.processSeq('t1', 'c1', 3, 60)
      const result = await store.cancelSlot('t1', 'c1', 3)
      expect(result).toBe('cancelled')
    })

    it('returns already_triggered for non-existent slot', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      const result = await store.cancelSlot('t1', 'c1', 5)
      expect(result).toBe('already_triggered')
    })

    it('returns already_triggered when no state exists', async () => {
      const result = await store.cancelSlot('t1', 'c1', 0)
      expect(result).toBe('already_triggered')
    })

    it('after cancel, slot can be re-registered', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      await store.processSeq('t1', 'c1', 3, 60)
      await store.cancelSlot('t1', 'c1', 3)
      // Now re-register slot 3
      const result = await store.processSeq('t1', 'c1', 3, 60)
      expect(result).toEqual({ action: 'wait' })
    })
  })

  describe('getExpectedSeq', () => {
    it('returns null for uninitialized client', async () => {
      expect(await store.getExpectedSeq('t1', 'c1')).toBe(null)
    })
  })

  describe('cleanupSeq', () => {
    it('cleans up specific clientId', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      await store.processSeq('t1', 'c2', 0, 60)
      await store.cleanupSeq('t1', 'c1')
      expect(await store.getExpectedSeq('t1', 'c1')).toBe(null)
      expect(await store.getExpectedSeq('t1', 'c2')).toBe(1)
    })

    it('cleans up all clientIds for a task', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      await store.processSeq('t1', 'c2', 0, 60)
      await store.processSeq('t2', 'c1', 0, 60)
      await store.cleanupSeq('t1')
      expect(await store.getExpectedSeq('t1', 'c1')).toBe(null)
      expect(await store.getExpectedSeq('t1', 'c2')).toBe(null)
      expect(await store.getExpectedSeq('t2', 'c1')).toBe(1)
    })

    it('cleanup is idempotent', async () => {
      await store.processSeq('t1', 'c1', 0, 60)
      await store.cleanupSeq('t1', 'c1')
      await store.cleanupSeq('t1', 'c1') // second call should not throw
      expect(await store.getExpectedSeq('t1', 'c1')).toBe(null)
    })

    it('cleanup with no matching keys is no-op', async () => {
      await store.cleanupSeq('nonexistent')
      await store.cleanupSeq('nonexistent', 'c1')
      // Should not throw
    })
  })
})

// ─── Engine seq ordering ──────────────────────────────────────────────────

describe('TaskEngine seq ordering', () => {
  it('publishes events in order without clientId/clientSeq (existing behavior)', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)
    const e1 = await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {} })
    const e2 = await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {} })
    expect(e1.index).toBe(1)
    expect(e2.index).toBe(2)
    expect(e1.clientId).toBeUndefined()
    expect(e1.clientSeq).toBeUndefined()
  })

  it('accepts in-order events with clientId/clientSeq', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)
    const e0 = await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    const e1 = await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    expect(e0.clientId).toBe('c1')
    expect(e0.clientSeq).toBe(0)
    expect(e1.clientSeq).toBe(1)
    expect(e0.index).toBeLessThan(e1.index)
  })

  it('holds out-of-order event and releases when gap fills', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 5000 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'init', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const holdPromise = engine.publishEvent(task.id, { type: 'third', level: 'info', data: {}, clientId: 'c1', clientSeq: 2 })
    await new Promise(r => setTimeout(r, 10))

    const e1 = await engine.publishEvent(task.id, { type: 'second', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    const e2 = await holdPromise

    expect(e1.index).toBeLessThan(e2.index)
    expect(e1.clientSeq).toBe(1)
    expect(e2.clientSeq).toBe(2)
  })

  it('handles multiple out-of-order events with chain trigger', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 5000 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'e0', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const p3 = engine.publishEvent(task.id, { type: 'e3', level: 'info', data: {}, clientId: 'c1', clientSeq: 3 })
    const p4 = engine.publishEvent(task.id, { type: 'e4', level: 'info', data: {}, clientId: 'c1', clientSeq: 4 })
    const p2 = engine.publishEvent(task.id, { type: 'e2', level: 'info', data: {}, clientId: 'c1', clientSeq: 2 })
    await new Promise(r => setTimeout(r, 10))

    const e1 = await engine.publishEvent(task.id, { type: 'e1', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    const [e2, e3, e4] = await Promise.all([p2, p3, p4])

    expect(e1.index).toBeLessThan(e2.index)
    expect(e2.index).toBeLessThan(e3.index)
    expect(e3.index).toBeLessThan(e4.index)
  })

  it('rejects stale seq', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })

    await expect(
      engine.publishEvent(task.id, { type: 'c', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    ).rejects.toThrow(SeqStaleError)
  })

  it('SeqStaleError has correct fields', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    try {
      await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
      expect.unreachable('should have thrown')
    } catch (err) {
      expect(err).toBeInstanceOf(SeqStaleError)
      const e = err as SeqStaleError
      expect(e.receivedSeq).toBe(0)
      expect(e.expectedSeq).toBe(1)
      expect(e.name).toBe('SeqStaleError')
    }
  })

  it('rejects duplicate registered seq', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 5000 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 3 })
    await new Promise(r => setTimeout(r, 10))

    await expect(
      engine.publishEvent(task.id, { type: 'c', level: 'info', data: {}, clientId: 'c1', clientSeq: 3 })
    ).rejects.toThrow(SeqDuplicateError)
  })

  it('SeqDuplicateError has correct fields', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 5000 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 3 })
    await new Promise(r => setTimeout(r, 10))

    try {
      await engine.publishEvent(task.id, { type: 'c', level: 'info', data: {}, clientId: 'c1', clientSeq: 3 })
      expect.unreachable('should have thrown')
    } catch (err) {
      expect(err).toBeInstanceOf(SeqDuplicateError)
      const e = err as SeqDuplicateError
      expect(e.seq).toBe(3)
      expect(e.name).toBe('SeqDuplicateError')
    }
  })

  it('fast-fail mode rejects immediately on gap', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    await expect(
      engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 5, seqMode: 'fast-fail' })
    ).rejects.toThrow(SeqGapError)
  })

  it('SeqGapError has correct fields', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    try {
      await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 5, seqMode: 'fast-fail' })
      expect.unreachable('should have thrown')
    } catch (err) {
      expect(err).toBeInstanceOf(SeqGapError)
      const e = err as SeqGapError
      expect(e.receivedSeq).toBe(5)
      expect(e.expectedSeq).toBe(1)
      expect(e.name).toBe('SeqGapError')
    }
  })

  it('times out when gap is never filled', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 50 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    await expect(
      engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 5 })
    ).rejects.toThrow(SeqTimeoutError)
  }, 10_000)

  it('SeqTimeoutError has correct fields', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 50 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    try {
      await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 5 })
      expect.unreachable('should have thrown')
    } catch (err) {
      expect(err).toBeInstanceOf(SeqTimeoutError)
      const e = err as SeqTimeoutError
      expect(e.seq).toBe(5)
      expect(e.expectedSeq).toBe(1)
      expect(e.name).toBe('SeqTimeoutError')
    }
  }, 10_000)

  it('independent clientIds do not interfere', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    const e1 = await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    const e2 = await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c2', clientSeq: 0 })
    const e3 = await engine.publishEvent(task.id, { type: 'c', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    const e4 = await engine.publishEvent(task.id, { type: 'd', level: 'info', data: {}, clientId: 'c2', clientSeq: 1 })

    expect(e1.clientId).toBe('c1')
    expect(e2.clientId).toBe('c2')
    expect(e3.clientSeq).toBe(1)
    expect(e4.clientSeq).toBe(1)
  })

  it('supports arbitrary start seq', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    const e = await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 42 })
    expect(e.clientSeq).toBe(42)
    expect(await engine.getExpectedSeq(task.id, 'c1')).toBe(43)
  })

  it('getExpectedSeq returns null for unknown clientId', async () => {
    const { engine } = makeEngine()
    expect(await engine.getExpectedSeq('unknown-task', 'c1')).toBe(null)
  })

  it('events without seq mixed with seq events work independently', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    const e1 = await engine.publishEvent(task.id, { type: 'no-seq', level: 'info', data: {} })
    expect(e1.clientId).toBeUndefined()

    const e2 = await engine.publishEvent(task.id, { type: 'with-seq', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    expect(e2.clientId).toBe('c1')

    const e3 = await engine.publishEvent(task.id, { type: 'no-seq-2', level: 'info', data: {} })
    expect(e3.clientId).toBeUndefined()
  })

  it('timeout race: signal arrives at timeout boundary → success', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 100 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const holdPromise = engine.publishEvent(task.id, { type: 'c', level: 'info', data: {}, clientId: 'c1', clientSeq: 2 })

    setTimeout(async () => {
      await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    }, 50)

    const e2 = await holdPromise
    expect(e2.clientSeq).toBe(2)
  }, 10_000)

  it('timeout race: cancelSlot returns already_triggered → still succeeds', async () => {
    // To deterministically test the already_triggered path, we use a custom store
    // that intercepts cancelSlot to always return 'already_triggered'
    const realStore = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()

    // Wrap the store to intercept cancelSlot
    const wrappedStore = new Proxy(realStore, {
      get(target, prop, receiver) {
        if (prop === 'cancelSlot') {
          // Always return 'already_triggered' to simulate the race
          return async () => 'already_triggered' as const
        }
        return Reflect.get(target, prop, receiver)
      },
    })

    const engine = new TaskEngine({
      shortTermStore: wrappedStore,
      broadcast,
      seqHoldTimeout: 50, // short timeout to trigger the race
    })

    const task = await createRunningTask(engine)
    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    // seq=2 will wait, timeout fires, cancelSlot returns already_triggered → success path
    const event = await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 2 })
    expect(event.clientSeq).toBe(2)
    expect(event.type).toBe('b')
  }, 10_000)

  it('terminal state cleans up seq state', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    expect(await engine.getExpectedSeq(task.id, 'c1')).toBe(1)

    await engine.transitionTask(task.id, 'completed')
    await new Promise(r => setTimeout(r, 10))
    expect(await engine.getExpectedSeq(task.id, 'c1')).toBe(null)
  })

  it('terminal state cleans up all clientIds for the task', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    await engine.publishEvent(task.id, { type: 'b', level: 'info', data: {}, clientId: 'c2', clientSeq: 0 })

    await engine.transitionTask(task.id, 'failed', { error: { message: 'test' } })
    await new Promise(r => setTimeout(r, 10))
    expect(await engine.getExpectedSeq(task.id, 'c1')).toBe(null)
    expect(await engine.getExpectedSeq(task.id, 'c2')).toBe(null)
  })

  it('broadcast receives events in correct seq order', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 5000 })
    const task = await createRunningTask(engine)

    const received: TaskEvent[] = []
    engine.subscribe(task.id, (e) => {
      if (e.type.startsWith('e')) received.push(e)
    })

    await engine.publishEvent(task.id, { type: 'e0', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const p2 = engine.publishEvent(task.id, { type: 'e2', level: 'info', data: {}, clientId: 'c1', clientSeq: 2 })
    await new Promise(r => setTimeout(r, 10))
    await engine.publishEvent(task.id, { type: 'e1', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    await p2

    expect(received.map(e => e.type)).toEqual(['e0', 'e1', 'e2'])
  })

  it('getHistory returns events in correct index order after out-of-order publish', async () => {
    const { engine } = makeEngine({ seqHoldTimeout: 5000 })
    const task = await createRunningTask(engine)

    await engine.publishEvent(task.id, { type: 'e0', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const p2 = engine.publishEvent(task.id, { type: 'e2', level: 'info', data: {}, clientId: 'c1', clientSeq: 2 })
    await new Promise(r => setTimeout(r, 10))
    await engine.publishEvent(task.id, { type: 'e1', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    await p2

    const events = await engine.getEvents(task.id)
    const userEvents = events.filter(e => e.type.startsWith('e'))
    expect(userEvents.map(e => e.type)).toEqual(['e0', 'e1', 'e2'])
    // Verify index ordering
    for (let i = 1; i < userEvents.length; i++) {
      expect(userEvents[i]!.index).toBeGreaterThan(userEvents[i - 1]!.index)
    }
  })

  describe('concurrent publish scenarios', () => {
    it('concurrent in-order publishes from different clients', async () => {
      const { engine } = makeEngine()
      const task = await createRunningTask(engine)

      // Simulate two clients publishing simultaneously
      const results = await Promise.all([
        engine.publishEvent(task.id, { type: 'c1-e0', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 }),
        engine.publishEvent(task.id, { type: 'c2-e0', level: 'info', data: {}, clientId: 'c2', clientSeq: 0 }),
        engine.publishEvent(task.id, { type: 'c1-e1', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 }),
        engine.publishEvent(task.id, { type: 'c2-e1', level: 'info', data: {}, clientId: 'c2', clientSeq: 1 }),
      ])

      // All should succeed
      expect(results).toHaveLength(4)
      // Each client's events should have correct clientSeq
      const c1Events = results.filter(e => e.clientId === 'c1')
      const c2Events = results.filter(e => e.clientId === 'c2')
      expect(c1Events.map(e => e.clientSeq)).toEqual([0, 1])
      expect(c2Events.map(e => e.clientSeq)).toEqual([0, 1])
    })

    it('concurrent publish: 10 events arrive simultaneously in reverse order', async () => {
      const { engine } = makeEngine({ seqHoldTimeout: 5000 })
      const task = await createRunningTask(engine)

      // First event to initialize
      await engine.publishEvent(task.id, { type: 'e0', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

      // Fire events 10 down to 1 simultaneously (reverse order)
      const promises: Promise<TaskEvent>[] = []
      for (let i = 10; i >= 2; i--) {
        promises.push(engine.publishEvent(task.id, { type: `e${i}`, level: 'info', data: {}, clientId: 'c1', clientSeq: i }))
      }
      await new Promise(r => setTimeout(r, 20))

      // Send seq=1 to trigger the cascade
      const e1 = await engine.publishEvent(task.id, { type: 'e1', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
      const results = await Promise.all(promises)

      // All resolved
      expect(results).toHaveLength(9)
      // All events emitted in correct seq order
      const allEvents = [e1, ...results].sort((a, b) => a.index - b.index)
      for (let i = 1; i < allEvents.length; i++) {
        expect(allEvents[i]!.clientSeq).toBeGreaterThan(allEvents[i - 1]!.clientSeq!)
      }
    })

    it('multiple gaps filled one by one', async () => {
      const { engine } = makeEngine({ seqHoldTimeout: 5000 })
      const task = await createRunningTask(engine)

      await engine.publishEvent(task.id, { type: 'e0', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

      // Create gaps: send 3 and 5 (gaps at 1, 2, 4)
      const p3 = engine.publishEvent(task.id, { type: 'e3', level: 'info', data: {}, clientId: 'c1', clientSeq: 3 })
      const p5 = engine.publishEvent(task.id, { type: 'e5', level: 'info', data: {}, clientId: 'c1', clientSeq: 5 })
      await new Promise(r => setTimeout(r, 10))

      // Fill gap at 1 → triggers nothing (2 not registered)
      await engine.publishEvent(task.id, { type: 'e1', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })

      // Fill gap at 2 → triggers 3
      await engine.publishEvent(task.id, { type: 'e2', level: 'info', data: {}, clientId: 'c1', clientSeq: 2 })
      const e3 = await p3

      // 4 is still missing, 5 still held
      const p4 = engine.publishEvent(task.id, { type: 'e4', level: 'info', data: {}, clientId: 'c1', clientSeq: 4 })
      const [e4, e5] = await Promise.all([p4, p5])

      expect(e3.index).toBeLessThan(e4.index)
      expect(e4.index).toBeLessThan(e5.index)
    })
  })
})
