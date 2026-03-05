import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useDataStore } from '@/stores'

function levelBadgeVariant(
  level: string,
): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (level) {
    case 'debug':
      return 'secondary'
    case 'info':
      return 'default'
    case 'warn':
      return 'outline'
    case 'error':
      return 'destructive'
    default:
      return 'default'
  }
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  })
}

function truncateJson(data: unknown, maxLen = 80): string {
  if (data === undefined || data === null) return ''
  const str = JSON.stringify(data)
  if (str.length <= maxLen) return str
  return str.slice(0, maxLen) + '...'
}

export function EventHistory() {
  const { globalEvents } = useDataStore()

  if (globalEvents.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No events yet.
      </div>
    )
  }

  return (
    <ScrollArea className="h-full">
      <div className="space-y-0">
        {globalEvents.map((event) => (
          <div
            key={event.id}
            className={`flex items-center gap-2 border-b px-2 py-1.5 text-xs last:border-b-0 ${
              event.direction === 'sent'
                ? 'bg-blue-500/10'
                : event.direction === 'received'
                  ? 'bg-green-500/10'
                  : ''
            }`}
          >
            <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
              {formatTime(event.timestamp)}
            </span>
            {event.direction && (
              <span
                className={`shrink-0 font-bold text-[10px] ${
                  event.direction === 'sent' ? 'text-blue-400' : 'text-green-400'
                }`}
              >
                {event.sourceLabel ? `${event.sourceLabel} ` : ''}
                {event.direction === 'sent' ? '→' : '←'}
              </span>
            )}
            <Badge
              variant={levelBadgeVariant(event.level)}
              className="shrink-0 text-[10px] px-1.5 py-0"
            >
              {event.level}
            </Badge>
            <span className="shrink-0 font-mono text-[10px]">
              {event.type}
            </span>
            <span className="truncate font-mono text-[10px] text-muted-foreground">
              {truncateJson(event.data)}
            </span>
            {event.seriesId && (
              <Badge
                variant="outline"
                className="ml-auto shrink-0 text-[10px] px-1.5 py-0"
              >
                {event.seriesId}:{event.seriesMode ?? 'keep-all'}
              </Badge>
            )}
          </div>
        ))}
      </div>
    </ScrollArea>
  )
}
