import { describe, expect, it } from 'vitest'
import * as policy from '../../src/cleanup-policy.js'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore, MemoryLongTermStore } from '../../src/memory-adapters.js'
import type { CleanupRule, Task } from '../../src/types.js'

const success: CleanupRule = { match: { taskTypes: ['search.*'], status: ['completed', 'cancelled'] }, trigger: { afterMs: 86_400_000 }, target: 'events' }
const failure: CleanupRule = { match: { taskTypes: ['search.*'], status: ['failed'] }, trigger: { afterMs: 604_800_000 }, target: 'events' }
const config = () => policy.resolveCleanupConfig({ cleanup: { enabled: true, rules: [success, failure] } }, {})
const task = (): Task => ({ id: 'test', type: 'search.youtube', status: 'completed', createdAt: 0, updatedAt: 50_000, completedAt: 1_000, ...policy.resolveTaskCleanupPolicy('search.youtube', undefined, config(), 0) })

describe('terminal cleanup policy', () => {
  it('defaults off and honors an explicit boolean environment override', () => {
    expect(policy.resolveCleanupConfig({}, {})).toEqual({ enabled: false, rules: [] })
    expect(policy.resolveCleanupConfig({}, { TASKCAST_CLEANUP_ENABLED: 'true' }).enabled).toBe(true)
    expect(policy.resolveCleanupConfig({ cleanup: { enabled: true, rules: [success] } }, { TASKCAST_CLEANUP_ENABLED: 'false' }).enabled).toBe(false)
    expect(() => policy.resolveCleanupConfig({}, { TASKCAST_CLEANUP_ENABLED: 'yes' })).toThrow()
    expect(() => policy.resolveCleanupConfig({ cleanup: { enabled: 'true' } } as never, {})).toThrow()
  })

  it('uses completion time only for enrolled terminal tasks', () => {
    expect(policy.cleanupDeadline(task(), 'events')).toBe(86_401_000)
    expect(policy.cleanupDeadline({ ...task(), status: 'failed' }, 'events')).toBe(604_801_000)
    expect(policy.cleanupDeadline({ ...task(), status: 'cancelled' }, 'events')).toBe(86_401_000)
    expect(policy.cleanupDeadline({ ...task(), status: 'timeout' }, 'events')).toBeNull()
    expect(policy.cleanupDeadline({ ...task(), completedAt: undefined }, 'events')).toBeNull()
    expect(policy.cleanupDeadline({ ...task(), cleanupPolicyVersion: undefined }, 'events')).toBeNull()
    expect(policy.cleanupDeadline({ ...task(), cleanup: undefined }, 'events')).toBeNull()
    expect(policy.cleanupDeadline(task(), 'all')).toBeNull()
  })

  it.each(['pending', 'assigned', 'running', 'paused', 'blocked'] as const)('never schedules %s tasks', (status) => {
    expect(policy.cleanupDeadline({ ...task(), status }, 'events')).toBeNull()
  })

  it('freezes matching defaults and lets whole-task overrides or empty rules replace them', () => {
    const defaults = config()
    const resolved = policy.resolveTaskCleanupPolicy('search.youtube', undefined, defaults, 123)!
    defaults.rules[0]!.trigger.afterMs = 1
    expect(resolved.cleanup.rules[0]!.trigger.afterMs).toBe(86_400_000)
    expect(resolved.cleanupResolvedAt).toBe(123)
    expect(resolved.cleanupPolicyVersion).toBe(1)
    expect(policy.resolveTaskCleanupPolicy('other', undefined, config(), 0)?.cleanup.rules).toEqual([])
    expect(policy.resolveTaskCleanupPolicy(undefined, undefined, config(), 0)?.cleanup.rules).toEqual([])
    expect(policy.resolveTaskCleanupPolicy('search.youtube', { rules: [] }, config(), 0)?.cleanup.rules).toEqual([])
    const override: CleanupRule = { trigger: {}, target: 'all' }
    expect(policy.resolveTaskCleanupPolicy('search.youtube', { rules: [override] }, config(), 0)?.cleanup.rules).toEqual([override])
    expect(policy.resolveTaskCleanupPolicy('search.youtube', undefined, { enabled: false, rules: [] }, 0)).toBeUndefined()
  })

  it('applies the earliest matching rule independently for events and all', () => {
    const rules: CleanupRule[] = [success, { trigger: { afterMs: 500 }, target: 'events' }, { trigger: { afterMs: 900 }, target: 'all' }]
    expect(policy.cleanupDeadline({ ...task(), cleanup: { rules } }, 'events')).toBe(1_500)
    expect(policy.cleanupDeadline({ ...task(), cleanup: { rules } }, 'all')).toBe(1_900)
    expect(policy.cleanupDeadline({ ...task(), cleanup: { rules: [{ trigger: {}, target: 'events' }] } }, 'events')).toBe(1_000)
  })

  it.each([
    { target: 'task', trigger: {} },
    { target: 'events', trigger: {}, eventFilter: {} },
    { target: 'events', trigger: { afterMs: -1 } },
    { target: 'events', trigger: { afterMs: Infinity } },
    { target: 'events', trigger: { afterMs: 0.5 } },
    { target: 'events', trigger: { afterMs: Number.MAX_SAFE_INTEGER + 1 } },
    { target: 'events', trigger: {}, match: { status: ['typo'] } },
  ])('rejects unsupported enabled policy %j but leaves disabled legacy config alone', (rule) => {
    expect(() => policy.resolveCleanupConfig({ cleanup: { enabled: true, rules: [rule] } }, {})).toThrow()
    expect(() => policy.resolveCleanupConfig({ cleanup: { rules: [rule] } }, {})).not.toThrow()
    expect(() => policy.resolveTaskCleanupPolicy('search.youtube', { rules: [rule] } as never, config(), 0)).toThrow()
  })

  it('does not schedule invalid or overflowing persisted completion times', () => {
    for (const completedAt of [-1, NaN, Infinity, Number.MAX_SAFE_INTEGER]) {
      expect(policy.cleanupDeadline({ ...task(), completedAt }, 'events')).toBeNull()
    }
  })

  it('enrolls new engine tasks only, without retaining mutable caller rules', async () => {
    const store = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const old = new TaskEngine({ shortTermStore: store, longTermStore: durable, broadcast: new MemoryBroadcastProvider() })
    await old.createTask({ id: 'legacy', cleanup: { rules: [success] } })
    const defaults = config()
    const engine = new TaskEngine({ shortTermStore: store, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: defaults })
    const created = await engine.createTask({ type: 'search.youtube' })
    defaults.rules[0]!.trigger.afterMs = 1
    expect(created.cleanupPolicyVersion).toBe(1)
    expect(created.cleanup!.rules[0]!.trigger.afterMs).toBe(86_400_000)
    expect((await engine.getTask('legacy'))!.cleanupPolicyVersion).toBeUndefined()
    const override = { rules: [] }
    expect((await engine.createTask({ cleanup: override })).cleanup!.rules).toEqual([])
  })
})
