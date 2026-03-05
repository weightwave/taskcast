import { useEffect, useRef } from 'react'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useDataStore } from '@/stores'
import { useConnectionStore } from '@/stores'

function statusBadgeVariant(
  status: string,
): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (status) {
    case 'running':
      return 'default'
    case 'completed':
      return 'secondary'
    case 'failed':
    case 'timeout':
      return 'destructive'
    case 'cancelled':
      return 'outline'
    default:
      return 'outline'
  }
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

const TERMINAL_STATUSES = new Set(['completed', 'failed', 'cancelled', 'timeout'])

export function TaskList() {
  const { tasks, updateTask } = useDataStore()
  const baseUrl = useConnectionStore((s) => s.baseUrl)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  // Poll each known task every 3 seconds
  useEffect(() => {
    if (intervalRef.current) {
      clearInterval(intervalRef.current)
    }

    intervalRef.current = setInterval(async () => {
      const currentTasks = useDataStore.getState().tasks
      const token = useConnectionStore.getState().token

      const headers: HeadersInit = {}
      if (token) {
        headers['Authorization'] = `Bearer ${token}`
      }

      const pollableTasks = currentTasks.filter(
        (task) => !TERMINAL_STATUSES.has(task.status),
      )

      const results = await Promise.allSettled(
        pollableTasks.map(async (task) => {
          const res = await fetch(`${baseUrl}/tasks/${task.id}`, { headers })
          if (res.ok) {
            const updated = await res.json()
            updateTask(updated)
          }
        }),
      )

      // Silently ignore individual fetch failures
      void results
    }, 3000)

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current)
      }
    }
  }, [baseUrl, updateTask])

  if (tasks.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No tasks yet. Create one from a Backend panel.
      </div>
    )
  }

  return (
    <ScrollArea className="h-full">
      <table className="w-full text-xs">
        <thead>
          <tr className="border-b text-left text-muted-foreground">
            <th className="px-2 py-1.5 font-medium">ID</th>
            <th className="px-2 py-1.5 font-medium">Type</th>
            <th className="px-2 py-1.5 font-medium">Status</th>
            <th className="px-2 py-1.5 font-medium">Worker</th>
            <th className="px-2 py-1.5 font-medium">Created</th>
          </tr>
        </thead>
        <tbody>
          {tasks.map((task) => (
            <tr key={task.id} className="border-b last:border-b-0">
              <td className="px-2 py-1.5 font-mono">
                {task.id.slice(-8)}
              </td>
              <td className="px-2 py-1.5">{task.type ?? '-'}</td>
              <td className="px-2 py-1.5">
                <Badge
                  variant={statusBadgeVariant(task.status)}
                  className="text-[10px] px-1.5 py-0"
                >
                  {task.status}
                </Badge>
              </td>
              <td className="px-2 py-1.5 text-muted-foreground">
                {task.assignedWorker ?? '-'}
              </td>
              <td className="px-2 py-1.5 text-muted-foreground">
                {formatTime(task.createdAt)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </ScrollArea>
  )
}
