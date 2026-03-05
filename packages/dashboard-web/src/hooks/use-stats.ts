import { useMemo } from 'react'
import type { DashboardTask, Worker } from '@/types'
import { useTasksQuery } from './use-tasks'
import { useWorkersQuery } from './use-workers'

export function useStats() {
  const { data: tasks = [], isPending: isTasksPending } = useTasksQuery()
  const { data: workers = [], isPending: isWorkersPending } = useWorkersQuery()

  const stats = useMemo(() => {
    const statusCounts: Record<string, number> = {}
    for (const task of tasks) {
      statusCounts[task.status] = (statusCounts[task.status] ?? 0) + 1
    }

    const totalCapacity = workers.reduce((sum: number, w: Worker) => sum + (w.capacity ?? 0), 0)
    const usedCapacity = workers.reduce((sum: number, w: Worker) => sum + (w.usedSlots ?? 0), 0)
    const onlineWorkers = workers.filter((w: Worker) => w.status !== 'offline').length

    const recentTasks = [...tasks]
      .sort((a: DashboardTask, b: DashboardTask) => b.createdAt - a.createdAt)
      .slice(0, 10)

    return {
      statusCounts,
      totalTasks: tasks.length,
      onlineWorkers,
      totalCapacity,
      usedCapacity,
      recentTasks,
    }
  }, [tasks, workers])

  return {
    ...stats,
    isPending: isTasksPending || isWorkersPending,
  }
}
