import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { levelBadgeClass } from '@/lib/status'

interface TaskEvent {
  type?: string
  level?: string
  rawIndex?: number
  data?: unknown
}

export function EventTimeline({ events }: { events: unknown[] }) {
  const typedEvents = events as TaskEvent[]

  if (typedEvents.length === 0) {
    return <p className="text-sm text-muted-foreground">No events yet.</p>
  }

  return (
    <ScrollArea className="h-[400px]">
      <div className="space-y-1 pr-4">
        {typedEvents.map((event, i) => (
          <EventRow key={i} event={event} />
        ))}
      </div>
    </ScrollArea>
  )
}

function EventRow({ event }: { event: TaskEvent }) {
  const [expanded, setExpanded] = useState(false)
  const level = event.level ?? 'info'

  return (
    <div
      className="rounded-md border px-3 py-2 text-sm cursor-pointer hover:bg-muted/50 transition-colors"
      onClick={() => setExpanded(!expanded)}
    >
      <div className="flex items-center gap-2">
        <Badge variant="outline" className={cn('text-[10px] font-mono', levelBadgeClass(level))}>
          {level}
        </Badge>
        <span className="font-mono text-xs text-muted-foreground">{event.type ?? 'unknown'}</span>
        {event.rawIndex != null && (
          <span className="ml-auto text-xs text-muted-foreground">#{event.rawIndex}</span>
        )}
      </div>
      {expanded && event.data != null && (
        <pre className="mt-2 max-h-60 overflow-auto rounded bg-muted p-2 text-xs">
          {typeof event.data === 'string' ? event.data : JSON.stringify(event.data, null, 2)}
        </pre>
      )}
    </div>
  )
}
