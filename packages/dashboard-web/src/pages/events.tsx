import { useState, useMemo, useCallback } from 'react'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { useEventStream } from '@/hooks/use-events'
import { EventTimeline } from '@/components/tasks/event-timeline'
import { cn } from '@/lib/utils'
import { levelBadgeClass } from '@/lib/status'

const ALL_LEVELS = ['debug', 'info', 'warn', 'error'] as const

export function EventsPage() {
  const [taskIdInput, setTaskIdInput] = useState('')
  const [subscribedTaskId, setSubscribedTaskId] = useState<string | null>(null)
  const [typeFilter, setTypeFilter] = useState('')
  const [activeLevels, setActiveLevels] = useState<Set<string>>(new Set(ALL_LEVELS))

  const filter = useMemo(() => {
    const f: { types?: string; levels?: string } = {}
    if (typeFilter.trim()) f.types = typeFilter.trim()
    if (activeLevels.size < ALL_LEVELS.length && activeLevels.size > 0) {
      f.levels = Array.from(activeLevels).join(',')
    }
    return Object.keys(f).length > 0 ? f : undefined
  }, [typeFilter, activeLevels])

  const { events, isDone, doneReason, error } = useEventStream(subscribedTaskId, filter)

  const toggleLevel = useCallback((level: string) => {
    setActiveLevels((prev) => {
      const next = new Set(prev)
      if (next.has(level)) {
        next.delete(level)
      } else {
        next.add(level)
      }
      return next
    })
  }, [])

  function handleSubscribe() {
    if (taskIdInput.trim()) {
      setSubscribedTaskId(taskIdInput.trim())
    }
  }

  function handleUnsubscribe() {
    setSubscribedTaskId(null)
  }

  // Connection status
  let connectionStatus: { label: string; className: string }
  if (!subscribedTaskId) {
    connectionStatus = { label: 'Idle', className: 'text-muted-foreground' }
  } else if (error) {
    connectionStatus = { label: 'Error', className: 'text-red-600 dark:text-red-400' }
  } else if (isDone) {
    connectionStatus = { label: `Done (${doneReason ?? 'finished'})`, className: 'text-yellow-600 dark:text-yellow-400' }
  } else {
    connectionStatus = { label: 'Streaming', className: 'text-green-600 dark:text-green-400' }
  }

  return (
    <div className="flex h-full flex-col gap-4">
      <h2 className="text-2xl font-bold tracking-tight">Events</h2>

      {/* Connection controls */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">SSE Subscription</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex items-center gap-3">
            <Input
              placeholder="Task ID"
              value={taskIdInput}
              onChange={(e) => setTaskIdInput(e.target.value)}
              className="max-w-[400px]"
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleSubscribe()
              }}
            />
            {!subscribedTaskId ? (
              <Button onClick={handleSubscribe} disabled={!taskIdInput.trim()}>
                Subscribe
              </Button>
            ) : (
              <Button variant="destructive" onClick={handleUnsubscribe}>
                Unsubscribe
              </Button>
            )}
          </div>

          {/* Filters */}
          <div className="flex items-center gap-3 flex-wrap">
            <Input
              placeholder="Type filter (e.g. llm.*)"
              value={typeFilter}
              onChange={(e) => setTypeFilter(e.target.value)}
              className="max-w-[200px]"
            />

            <div className="flex items-center gap-1">
              {ALL_LEVELS.map((level) => (
                <Badge
                  key={level}
                  variant="outline"
                  className={cn(
                    'cursor-pointer select-none transition-opacity',
                    activeLevels.has(level) ? levelBadgeClass(level) : 'opacity-30',
                  )}
                  onClick={() => toggleLevel(level)}
                >
                  {level}
                </Badge>
              ))}
            </div>
          </div>

          {/* Status bar */}
          <div className="flex items-center justify-between text-sm">
            <span>
              Status: <span className={cn('font-medium', connectionStatus.className)}>{connectionStatus.label}</span>
            </span>
            <span className="text-muted-foreground">{events.length} event(s)</span>
          </div>

          {error && (
            <p className="text-sm text-destructive">{error.message}</p>
          )}
        </CardContent>
      </Card>

      {/* Event stream */}
      <div className="min-h-0 flex-1">
        <EventTimeline events={events} />
      </div>
    </div>
  )
}
