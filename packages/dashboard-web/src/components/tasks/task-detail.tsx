import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Separator } from '@/components/ui/separator'
import { Skeleton } from '@/components/ui/skeleton'
import { useTaskQuery } from '@/hooks/use-tasks'
import { useEventStream } from '@/hooks/use-events'
import { formatRelativeTime } from '@/lib/utils'
import { statusBadgeVariant } from '@/lib/status'
import { TaskActions } from './task-actions'
import { EventTimeline } from './event-timeline'
import type { DashboardTask } from '@/types'

export function TaskDetail({ taskId }: { taskId: string }) {
  const { data, isLoading } = useTaskQuery(taskId)
  const { events } = useEventStream(taskId)

  if (isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-8 w-3/4" />
        <Skeleton className="h-32 w-full" />
        <Skeleton className="h-32 w-full" />
      </div>
    )
  }

  if (!data) {
    return <p className="text-sm text-muted-foreground">Task not found.</p>
  }

  const task = data as DashboardTask

  return (
    <div className="space-y-4 overflow-auto">
      {/* Header */}
      <div className="flex items-center gap-3 flex-wrap">
        <h3 className="text-lg font-semibold font-mono">{task.id}</h3>
        <Badge variant={statusBadgeVariant(task.status)}>{task.status}</Badge>
        <Badge variant={task.hot ? 'default' : 'outline'} className="text-xs">
          {task.hot ? 'Hot' : 'Cold'}
        </Badge>
        {task.subscriberCount != null && (
          <span className="text-xs text-muted-foreground">{task.subscriberCount} subscriber(s)</span>
        )}
      </div>

      {/* Info Card */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Info</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2 text-sm">
          <InfoRow label="Type" value={task.type ?? '-'} />
          <InfoRow label="TTL" value={task.ttl != null ? `${task.ttl}s` : '-'} />
          <InfoRow label="Worker" value={task.workerId ? task.workerId : '-'} mono={!!task.workerId} />
          <InfoRow label="Created" value={task.createdAt ? formatRelativeTime(task.createdAt) : '-'} />
          {task.updatedAt && <InfoRow label="Updated" value={formatRelativeTime(task.updatedAt)} />}
          {task.completedAt && <InfoRow label="Completed" value={formatRelativeTime(task.completedAt)} />}
        </CardContent>
      </Card>

      {/* Params */}
      {task.params != null && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Params</CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="max-h-48 overflow-auto rounded bg-muted p-3 text-xs">
              {JSON.stringify(task.params, null, 2)}
            </pre>
          </CardContent>
        </Card>
      )}

      {/* Result */}
      {task.result != null && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Result</CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="max-h-48 overflow-auto rounded bg-muted p-3 text-xs">
              {JSON.stringify(task.result, null, 2)}
            </pre>
          </CardContent>
        </Card>
      )}

      {/* Error */}
      {task.error != null && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm text-destructive">Error</CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="max-h-48 overflow-auto rounded bg-destructive/10 p-3 text-xs text-destructive">
              {JSON.stringify(task.error, null, 2)}
            </pre>
          </CardContent>
        </Card>
      )}

      {/* Actions */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Actions</CardTitle>
        </CardHeader>
        <CardContent>
          <TaskActions taskId={task.id} currentStatus={task.status} />
        </CardContent>
      </Card>

      <Separator />

      {/* Event Timeline */}
      <div>
        <h4 className="mb-3 text-sm font-semibold">Events ({events.length})</h4>
        <EventTimeline events={events} />
      </div>
    </div>
  )
}

function InfoRow({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex justify-between">
      <span className="text-muted-foreground">{label}</span>
      <span className={mono ? 'font-mono' : ''}>{value}</span>
    </div>
  )
}
