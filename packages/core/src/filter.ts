import type { TaskEvent, SubscribeFilter, Level } from './types.js'

export interface FilteredEvent {
  filteredIndex: number
  rawIndex: number
  event: TaskEvent
}

export function matchesType(type: string, patterns: string[] | undefined): boolean {
  if (patterns === undefined) return true
  if (patterns.length === 0) return false
  return patterns.some((pattern) => {
    if (pattern === '*') return true
    if (pattern.endsWith('.*')) {
      const prefix = pattern.slice(0, -2)
      // 'llm.*' matches 'llm.delta', 'llm.delta.chunk' but NOT 'llm'
      return type.startsWith(prefix + '.')
    }
    return type === pattern
  })
}

export function matchesFilter(event: TaskEvent, filter: SubscribeFilter): boolean {
  const includeStatus = filter.includeStatus ?? true

  if (!includeStatus && event.type === 'taskcast:status') {
    return false
  }

  if (filter.types !== undefined && !matchesType(event.type, filter.types)) {
    return false
  }

  if (filter.levels !== undefined && !filter.levels.includes(event.level as Level)) {
    return false
  }

  return true
}

export function applyFilteredIndex(
  events: TaskEvent[],
  filter: SubscribeFilter,
): FilteredEvent[] {
  const since = filter.since

  let filteredCounter = 0
  const result: FilteredEvent[] = []

  for (const event of events) {
    if (!matchesFilter(event, filter)) continue

    const currentFilteredIndex = filteredCounter
    filteredCounter++

    // since.index: skip events where filteredIndex <= since.index
    if (since?.index !== undefined && currentFilteredIndex <= since.index) {
      continue
    }

    result.push({
      filteredIndex: currentFilteredIndex,
      rawIndex: event.index,
      event,
    })
  }

  return result
}
