import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useDataStore } from '@/stores'

function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

function truncateJson(data: unknown, maxLen = 80): string {
  if (data === undefined || data === null) return ''
  const str = JSON.stringify(data)
  if (str.length <= maxLen) return str
  return str.slice(0, maxLen) + '...'
}

function statusCodeVariant(
  code: number | undefined,
): 'default' | 'destructive' {
  if (code !== undefined && code >= 200 && code < 300) return 'default'
  return 'destructive'
}

export function WebhookLogs() {
  const { webhookLogs } = useDataStore()

  if (webhookLogs.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No webhook deliveries yet. Register a webhook via the Backend panel to
        see deliveries here.
      </div>
    )
  }

  return (
    <ScrollArea className="h-full">
      <table className="w-full text-xs">
        <thead>
          <tr className="border-b text-left text-muted-foreground">
            <th className="px-2 py-1.5 font-medium">Time</th>
            <th className="px-2 py-1.5 font-medium">URL</th>
            <th className="px-2 py-1.5 font-medium">Status</th>
            <th className="px-2 py-1.5 font-medium">Payload</th>
          </tr>
        </thead>
        <tbody>
          {webhookLogs.map((log) => (
            <tr key={log.id} className="border-b last:border-b-0">
              <td className="px-2 py-1.5 text-muted-foreground whitespace-nowrap">
                {formatTime(log.timestamp)}
              </td>
              <td className="px-2 py-1.5 max-w-[200px] truncate font-mono">
                {log.url}
              </td>
              <td className="px-2 py-1.5">
                {log.statusCode !== undefined ? (
                  <Badge
                    variant={statusCodeVariant(log.statusCode)}
                    className="text-[10px] px-1.5 py-0"
                  >
                    {log.statusCode}
                  </Badge>
                ) : log.error ? (
                  <Badge variant="destructive" className="text-[10px] px-1.5 py-0">
                    error
                  </Badge>
                ) : (
                  '-'
                )}
              </td>
              <td className="px-2 py-1.5 max-w-[300px] truncate font-mono text-muted-foreground">
                {truncateJson(log.payload)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </ScrollArea>
  )
}
