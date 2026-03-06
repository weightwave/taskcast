import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { cn } from '@/lib/utils'

const STATUS_CONFIG: Record<string, { label: string; color: string }> = {
  pending: { label: 'Pending', color: 'text-yellow-600 bg-yellow-50 border-yellow-200 dark:text-yellow-400 dark:bg-yellow-950 dark:border-yellow-800' },
  running: { label: 'Running', color: 'text-blue-600 bg-blue-50 border-blue-200 dark:text-blue-400 dark:bg-blue-950 dark:border-blue-800' },
  completed: { label: 'Completed', color: 'text-green-600 bg-green-50 border-green-200 dark:text-green-400 dark:bg-green-950 dark:border-green-800' },
  failed: { label: 'Failed', color: 'text-red-600 bg-red-50 border-red-200 dark:text-red-400 dark:bg-red-950 dark:border-red-800' },
  timeout: { label: 'Timeout', color: 'text-orange-600 bg-orange-50 border-orange-200 dark:text-orange-400 dark:bg-orange-950 dark:border-orange-800' },
  cancelled: { label: 'Cancelled', color: 'text-gray-600 bg-gray-50 border-gray-200 dark:text-gray-400 dark:bg-gray-950 dark:border-gray-800' },
}

const STATUSES = ['pending', 'running', 'completed', 'failed', 'timeout', 'cancelled']

export function StatusCards({ statusCounts, totalTasks }: { statusCounts: Record<string, number>; totalTasks: number }) {
  return (
    <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
      {STATUSES.map((status) => {
        const config = STATUS_CONFIG[status]
        const count = statusCounts[status] ?? 0
        return (
          <Card key={status} className={cn('border', config.color)}>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium">{config.label}</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold">{count}</div>
              {totalTasks > 0 && (
                <p className="text-xs text-muted-foreground">
                  {Math.round((count / totalTasks) * 100)}% of total
                </p>
              )}
            </CardContent>
          </Card>
        )
      })}
    </div>
  )
}
