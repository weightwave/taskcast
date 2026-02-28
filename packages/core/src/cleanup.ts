import { matchesType } from './filter.js'
import { isTerminal } from './state-machine.js'
import type { Task, TaskEvent, CleanupRule, TaskStatus } from './types.js'

export function matchesCleanupRule(
  task: Task,
  rule: CleanupRule,
  now: number,
): boolean {
  if (!isTerminal(task.status)) return false

  if (rule.match?.status && !rule.match.status.includes(task.status)) {
    return false
  }

  if (rule.match?.taskTypes) {
    if (!task.type || !matchesType(task.type, rule.match.taskTypes)) {
      return false
    }
  }

  if (rule.trigger.afterMs !== undefined) {
    const completedAt = task.completedAt ?? task.updatedAt
    const elapsed = now - completedAt
    if (elapsed < rule.trigger.afterMs) return false
  }

  return true
}

export function filterEventsForCleanup(
  events: TaskEvent[],
  rule: CleanupRule,
  now: number,
  completedAt?: number,
): TaskEvent[] {
  const ef = rule.eventFilter
  if (!ef) return events

  return events.filter((event) => {
    if (ef.types && !matchesType(event.type, ef.types)) return false
    if (ef.levels && !ef.levels.includes(event.level)) return false
    if (ef.seriesMode && event.seriesMode && !ef.seriesMode.includes(event.seriesMode)) return false
    if (ef.olderThanMs !== undefined && completedAt !== undefined) {
      const cutoff = completedAt - ef.olderThanMs
      if (event.timestamp >= cutoff) return false
    }
    return true
  })
}
