import { z } from 'zod'
import type { TaskcastConfig } from './config.js'
import { matchesType } from './filter.js'
import { isTerminal } from './state-machine.js'
import type { CleanupRule, Task } from './types.js'

export interface ResolvedCleanupConfig { enabled: boolean; rules: CleanupRule[] }
export interface TaskCleanupPolicy { rules: CleanupRule[] }

export class CleanupPolicyError extends Error {
  constructor(message: string) { super(message); this.name = 'CleanupPolicyError' }
}

const ruleSchema = z.object({
  name: z.string().optional(),
  match: z.object({
    taskTypes: z.array(z.string()).optional(),
    status: z.array(z.enum(['pending', 'assigned', 'running', 'paused', 'blocked', 'completed', 'failed', 'timeout', 'cancelled'])).optional(),
  }).optional(),
  trigger: z.object({ afterMs: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER).optional() }),
  target: z.enum(['events', 'all']),
  eventFilter: z.never().optional(),
})

function parseRules(input: unknown): CleanupRule[] {
  const parsed = z.array(ruleSchema).safeParse(input)
  if (!parsed.success) throw new CleanupPolicyError(`Invalid cleanup policy: ${parsed.error.message}`)
  return parsed.data.map(rule => ({
    target: rule.target,
    trigger: rule.trigger.afterMs === undefined ? {} : { afterMs: rule.trigger.afterMs },
    ...(rule.name === undefined ? {} : { name: rule.name }),
    ...(rule.match === undefined ? {} : { match: {
      ...(rule.match.taskTypes === undefined ? {} : { taskTypes: rule.match.taskTypes }),
      ...(rule.match.status === undefined ? {} : { status: rule.match.status }),
    } }),
  }))
}

export function resolveCleanupConfig(
  config: TaskcastConfig,
  env: Record<string, string | undefined> = process.env,
): ResolvedCleanupConfig {
  const fileEnabled = config.cleanup?.enabled
  if (fileEnabled !== undefined && typeof fileEnabled !== 'boolean') {
    throw new Error('cleanup.enabled must be true or false')
  }
  const override = env['TASKCAST_CLEANUP_ENABLED']
  if (override !== undefined && override !== 'true' && override !== 'false') {
    throw new Error('TASKCAST_CLEANUP_ENABLED must be true or false')
  }
  const enabled = override === undefined ? fileEnabled ?? false : override === 'true'
  return { enabled, rules: enabled ? parseRules(config.cleanup?.rules ?? []) : [] }
}

export function resolveTaskCleanupPolicy(
  type: string | undefined,
  override: TaskCleanupPolicy | undefined,
  config: ResolvedCleanupConfig,
  now: number,
): { cleanup: TaskCleanupPolicy; cleanupPolicyVersion: 1; cleanupResolvedAt: number } | undefined {
  if (!config.enabled) return undefined
  const rules = parseRules(override?.rules ?? config.rules)
  return {
    cleanup: { rules: override === undefined ? rules.filter(rule => !rule.match?.taskTypes || (type !== undefined && matchesType(type, rule.match.taskTypes))) : rules },
    cleanupPolicyVersion: 1,
    cleanupResolvedAt: now,
  }
}

export function cleanupDeadline(task: Task, target: 'events' | 'all'): number | null {
  if (task.cleanupPolicyVersion !== 1 || !isTerminal(task.status)
    || task.completedAt === undefined || !Number.isSafeInteger(task.completedAt) || task.completedAt < 0) return null
  let earliest: number | null = null
  for (const rule of task.cleanup?.rules ?? []) {
    if (rule.target !== target || rule.eventFilter !== undefined) continue
    if (rule.match?.status && !rule.match.status.includes(task.status)) continue
    if (rule.match?.taskTypes && (!task.type || !matchesType(task.type, rule.match.taskTypes))) continue
    const after = rule.trigger.afterMs ?? 0
    const due = task.completedAt + after
    if (!Number.isSafeInteger(after) || after < 0 || !Number.isSafeInteger(due)) continue
    earliest = earliest === null ? due : Math.min(earliest, due)
  }
  return earliest
}
