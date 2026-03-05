import { useStats } from '@/hooks/use-stats'
import { StatusCards } from '@/components/overview/status-cards'
import { WorkerSummary } from '@/components/overview/worker-summary'
import { RecentTasks } from '@/components/overview/recent-tasks'
import { Skeleton } from '@/components/ui/skeleton'

export function OverviewPage() {
  const { statusCounts, totalTasks, onlineWorkers, totalCapacity, usedCapacity, recentTasks, isPending } = useStats()

  const isLoading = isPending

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-bold tracking-tight">Overview</h2>

      {isLoading ? (
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
          {Array.from({ length: 6 }).map((_, i) => (
            <Skeleton key={i} className="h-28 w-full rounded-xl" />
          ))}
        </div>
      ) : (
        <StatusCards statusCounts={statusCounts} totalTasks={totalTasks} />
      )}

      <div className="grid gap-6 lg:grid-cols-3">
        <div className="lg:col-span-1">
          <WorkerSummary
            onlineWorkers={onlineWorkers}
            totalCapacity={totalCapacity}
            usedCapacity={usedCapacity}
          />
        </div>
        <div className="lg:col-span-2">
          <RecentTasks tasks={recentTasks} />
        </div>
      </div>
    </div>
  )
}
