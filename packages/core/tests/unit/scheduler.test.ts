import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { TaskScheduler } from '../../src/scheduler.js'

function makeSetup(opts?: { pausedColdAfterMs?: number; blockedColdAfterMs?: number }) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const scheduler = new TaskScheduler({
    engine,
    shortTermStore: store,
    checkIntervalMs: 1000,
    ...opts,
  })
  return { store, broadcast, engine, scheduler }
}

describe('TaskScheduler', () => {
  // ─── Wake-Up Timer Tests ─────────────────────────────────────────────

  describe('_checkWakeUpTimers (via tick)', () => {
    it('auto-resumes blocked tasks with expired resumeAt', async () => {
      const { engine, scheduler } = makeSetup()

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'blocked', {
        resumeAfterMs: 1000,
      })

      // Verify task is blocked with a resumeAt
      const blockedTask = await engine.getTask(task.id)
      expect(blockedTask!.status).toBe('blocked')
      expect(blockedTask!.resumeAt).toBeDefined()

      // Advance time past the resumeAt
      vi.useFakeTimers()
      vi.setSystemTime(blockedTask!.resumeAt! + 1)

      await scheduler.tick()

      const resumed = await engine.getTask(task.id)
      expect(resumed!.status).toBe('running')

      vi.useRealTimers()
    })

    it('does not resume blocked tasks with future resumeAt', async () => {
      const { engine, scheduler } = makeSetup()

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'blocked', {
        resumeAfterMs: 60_000,
      })

      // Time is well before resumeAt
      await scheduler.tick()

      const stillBlocked = await engine.getTask(task.id)
      expect(stillBlocked!.status).toBe('blocked')
    })

    it('does not resume blocked tasks without resumeAt', async () => {
      const { engine, scheduler } = makeSetup()

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'blocked')

      const blockedTask = await engine.getTask(task.id)
      expect(blockedTask!.resumeAt).toBeUndefined()

      await scheduler.tick()

      const stillBlocked = await engine.getTask(task.id)
      expect(stillBlocked!.status).toBe('blocked')
    })

    it('handles transition errors gracefully (task already transitioned)', async () => {
      const { engine, scheduler } = makeSetup()

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'blocked', {
        resumeAfterMs: 1000,
      })

      const blockedTask = await engine.getTask(task.id)

      vi.useFakeTimers()
      vi.setSystemTime(blockedTask!.resumeAt! + 1)

      // Transition to cancelled before the scheduler tick
      await engine.transitionTask(task.id, 'cancelled')

      // tick should not throw even though blocked->running is no longer valid
      await expect(scheduler.tick()).resolves.toBeUndefined()

      const final = await engine.getTask(task.id)
      expect(final!.status).toBe('cancelled')

      vi.useRealTimers()
    })
  })

  // ─── Cold Demotion Tests ─────────────────────────────────────────────

  describe('_checkColdDemotion (via tick)', () => {
    it('emits taskcast:cold for old paused tasks', async () => {
      const { engine, scheduler } = makeSetup({ pausedColdAfterMs: 5000 })

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'paused')

      const pausedTask = await engine.getTask(task.id)

      vi.useFakeTimers()
      vi.setSystemTime(pausedTask!.updatedAt + 6000)

      const publishSpy = vi.spyOn(engine, 'publishEvent')

      await scheduler.tick()

      expect(publishSpy).toHaveBeenCalledWith(task.id, {
        type: 'taskcast:cold',
        level: 'info',
        data: {},
      })

      vi.useRealTimers()
    })

    it('emits taskcast:cold for old blocked tasks', async () => {
      const { engine, scheduler } = makeSetup({ blockedColdAfterMs: 5000 })

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'blocked')

      const blockedTask = await engine.getTask(task.id)

      vi.useFakeTimers()
      vi.setSystemTime(blockedTask!.updatedAt + 6000)

      const publishSpy = vi.spyOn(engine, 'publishEvent')

      await scheduler.tick()

      expect(publishSpy).toHaveBeenCalledWith(task.id, {
        type: 'taskcast:cold',
        level: 'info',
        data: {},
      })

      vi.useRealTimers()
    })

    it('does not emit cold event when paused task is not old enough', async () => {
      const { engine, scheduler } = makeSetup({ pausedColdAfterMs: 60_000 })

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'paused')

      const publishSpy = vi.spyOn(engine, 'publishEvent')

      // Time has not advanced enough
      await scheduler.tick()

      // publishEvent may have been called by transitionTask; filter for our specific call
      const coldCalls = publishSpy.mock.calls.filter(
        (call) => call[1].type === 'taskcast:cold',
      )
      expect(coldCalls).toHaveLength(0)
    })

    it('does not emit cold event when blocked task is not old enough', async () => {
      const { engine, scheduler } = makeSetup({ blockedColdAfterMs: 60_000 })

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'blocked')

      const publishSpy = vi.spyOn(engine, 'publishEvent')

      await scheduler.tick()

      const coldCalls = publishSpy.mock.calls.filter(
        (call) => call[1].type === 'taskcast:cold',
      )
      expect(coldCalls).toHaveLength(0)
    })

    it('does not emit cold event when thresholds are disabled', async () => {
      // No pausedColdAfterMs or blockedColdAfterMs set
      const { engine, scheduler } = makeSetup()

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'paused')

      const publishSpy = vi.spyOn(engine, 'publishEvent')

      vi.useFakeTimers()
      vi.setSystemTime(Date.now() + 999_999_999)

      await scheduler.tick()

      const coldCalls = publishSpy.mock.calls.filter(
        (call) => call[1].type === 'taskcast:cold',
      )
      expect(coldCalls).toHaveLength(0)

      vi.useRealTimers()
    })

    it('skips blocked tasks when only pausedColdAfterMs is set', async () => {
      const { engine, scheduler } = makeSetup({ pausedColdAfterMs: 5000 })

      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.transitionTask(task.id, 'blocked')

      const blockedTask = await engine.getTask(task.id)

      vi.useFakeTimers()
      vi.setSystemTime(blockedTask!.updatedAt + 60_000)

      const publishSpy = vi.spyOn(engine, 'publishEvent')

      await scheduler.tick()

      const coldCalls = publishSpy.mock.calls.filter(
        (call) => call[1].type === 'taskcast:cold',
      )
      expect(coldCalls).toHaveLength(0)

      vi.useRealTimers()
    })
  })

  // ─── Lifecycle Tests ─────────────────────────────────────────────────

  describe('start/stop lifecycle', () => {
    beforeEach(() => {
      vi.useFakeTimers()
    })

    afterEach(() => {
      vi.useRealTimers()
    })

    it('start() creates periodic interval that calls tick', async () => {
      const { scheduler } = makeSetup()
      const tickSpy = vi.spyOn(scheduler, 'tick').mockResolvedValue(undefined)

      scheduler.start()

      expect(tickSpy).not.toHaveBeenCalled()

      await vi.advanceTimersByTimeAsync(1000)
      expect(tickSpy).toHaveBeenCalledTimes(1)

      await vi.advanceTimersByTimeAsync(1000)
      expect(tickSpy).toHaveBeenCalledTimes(2)

      scheduler.stop()
    })

    it('stop() clears the interval so tick is no longer called', async () => {
      const { scheduler } = makeSetup()
      const tickSpy = vi.spyOn(scheduler, 'tick').mockResolvedValue(undefined)

      scheduler.start()

      await vi.advanceTimersByTimeAsync(1000)
      expect(tickSpy).toHaveBeenCalledTimes(1)

      scheduler.stop()

      await vi.advanceTimersByTimeAsync(5000)
      expect(tickSpy).toHaveBeenCalledTimes(1)
    })

    it('stop() is safe to call without start()', () => {
      const { scheduler } = makeSetup()
      expect(() => scheduler.stop()).not.toThrow()
    })
  })
})
