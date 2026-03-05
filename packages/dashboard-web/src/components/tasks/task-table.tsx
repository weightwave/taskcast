import { Badge } from '@/components/ui/badge'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { formatRelativeTime } from '@/lib/utils'
import { statusBadgeVariant } from '@/lib/status'
import type { DashboardTask } from '@/types'

export function TaskTable({
  tasks,
  selectedTaskId,
  onSelect,
}: {
  tasks: DashboardTask[]
  selectedTaskId: string | null
  onSelect: (taskId: string) => void
}) {
  if (tasks.length === 0) {
    return (
      <div className="flex h-40 items-center justify-center text-sm text-muted-foreground">
        No tasks found.
      </div>
    )
  }

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>ID</TableHead>
          <TableHead>Type</TableHead>
          <TableHead>Status</TableHead>
          <TableHead>Hot/Cold</TableHead>
          <TableHead>Subs</TableHead>
          <TableHead>Worker</TableHead>
          <TableHead>Created</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {tasks.map((task) => (
          <TableRow
            key={task.id}
            className="cursor-pointer"
            data-state={task.id === selectedTaskId ? 'selected' : undefined}
            onClick={() => onSelect(task.id)}
          >
            <TableCell className="font-mono text-xs">{task.id.slice(-8)}</TableCell>
            <TableCell className="text-sm">{task.type ?? '-'}</TableCell>
            <TableCell>
              <Badge variant={statusBadgeVariant(task.status)}>{task.status}</Badge>
            </TableCell>
            <TableCell>
              <Badge variant={task.hot ? 'default' : 'outline'} className="text-xs">
                {task.hot ? 'Hot' : 'Cold'}
              </Badge>
            </TableCell>
            <TableCell className="text-sm">{task.subscriberCount ?? 0}</TableCell>
            <TableCell className="font-mono text-xs">
              {task.workerId ? task.workerId.slice(-6) : '-'}
            </TableCell>
            <TableCell className="text-sm text-muted-foreground">
              {task.createdAt ? formatRelativeTime(task.createdAt) : '-'}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
