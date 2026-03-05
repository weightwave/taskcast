import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
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

interface RecentTask {
  id: string
  type?: string
  status: string
  createdAt: number
}

export function RecentTasks({ tasks }: { tasks: unknown[] }) {
  const typedTasks = tasks as RecentTask[]

  if (typedTasks.length === 0) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>Recent Tasks</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">No tasks yet.</p>
        </CardContent>
      </Card>
    )
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Recent Tasks</CardTitle>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>ID</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Created</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {typedTasks.map((task) => (
              <TableRow key={task.id}>
                <TableCell className="font-mono text-xs">{task.id.slice(-8)}</TableCell>
                <TableCell className="text-sm">{task.type ?? '-'}</TableCell>
                <TableCell>
                  <Badge variant={statusBadgeVariant(task.status)}>{task.status}</Badge>
                </TableCell>
                <TableCell className="text-sm text-muted-foreground">
                  {task.createdAt ? formatRelativeTime(task.createdAt) : '-'}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  )
}
