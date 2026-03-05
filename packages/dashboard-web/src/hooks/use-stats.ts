import { useMemo } from 'react'
import { useTasksQuery } from './use-tasks'
import { useWorkersQuery } from './use-workers'

export function useStats() {
  const { data: tasks = [] } = useTasksQuery()
  const { data: workers = [] } = useWorkersQuery()

  return useMemo(() => {
    const statusCounts: Record<string, number> = {}
    for (const task of tasks as Array<{ status: string }>) {
      statusCounts[task.status] = (statusCounts[task.status] ?? 0) + 1
    }

    const workerList = workers as Array<{ status: string; capacity: number; usedSlots: number }>
    const totalCapacity = workerList.reduce((sum, w) => sum + (w.capacity ?? 0), 0)
    const usedCapacity = workerList.reduce((sum, w) => sum + (w.usedSlots ?? 0), 0)
    const onlineWorkers = workerList.filter((w) => w.status !== 'offline').length

    return {
      statusCounts,
      totalTasks: tasks.length,
      onlineWorkers,
      totalCapacity,
      usedCapacity,
      recentTasks: (tasks as unknown[]).slice(0, 10),
    }
  }, [tasks, workers])
}
