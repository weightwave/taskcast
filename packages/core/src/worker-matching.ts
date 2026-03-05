import type { Task, TagMatcher, WorkerMatchRule } from './types.js'
import { matchesType } from './filter.js'

/**
 * Checks if a set of task tags matches a TagMatcher.
 *
 * - `all`: every tag in `all` must exist in taskTags
 * - `any`: at least one tag in `any` must exist in taskTags
 * - `none`: none of the tags in `none` may exist in taskTags
 * - Empty/undefined matcher matches everything
 * - Undefined taskTags treated as empty array
 * - Empty arrays in matcher fields are vacuous true
 */
export function matchesTag(taskTags: string[] | undefined, matcher: TagMatcher): boolean {
  const tags = taskTags ?? []

  if (matcher.all !== undefined && matcher.all.length > 0) {
    if (!matcher.all.every((tag) => tags.includes(tag))) return false
  }

  if (matcher.any !== undefined && matcher.any.length > 0) {
    if (!matcher.any.some((tag) => tags.includes(tag))) return false
  }

  if (matcher.none !== undefined && matcher.none.length > 0) {
    if (matcher.none.some((tag) => tags.includes(tag))) return false
  }

  return true
}

/**
 * Checks if a task matches a WorkerMatchRule.
 *
 * - If rule has `taskTypes`: task.type must match using wildcard matching
 * - If rule has `tags`: use matchesTag
 * - Both conditions are AND'd
 * - Empty/no rule matches everything
 * - Task with no `type` does not match if rule has `taskTypes`
 */
export function matchesWorkerRule(task: Task, rule: WorkerMatchRule): boolean {
  if (rule.taskTypes !== undefined && rule.taskTypes.length > 0) {
    if (task.type === undefined) return false
    if (!matchesType(task.type, rule.taskTypes)) return false
  }

  if (rule.tags !== undefined) {
    if (!matchesTag(task.tags, rule.tags)) return false
  }

  return true
}
