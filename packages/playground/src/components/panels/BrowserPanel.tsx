import { useState, useCallback, useRef, useEffect } from 'react'
import { TaskcastClient } from '@taskcast/client'
import type { SSEEnvelope, Level } from '@taskcast/core'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { usePanelStore, useDataStore } from '@/stores'
import type { Panel } from '@/stores'
import { useApi } from '@/hooks/useApi'
import { PanelAuthConfig } from '@/components/panels/PanelAuthConfig'

/* ------------------------------------------------------------------ */
/*  Shared: task ID selector (same pattern as BackendPanel)           */
/* ------------------------------------------------------------------ */

function TaskIdField({
  value,
  onChange,
}: {
  value: string
  onChange: (v: string) => void
}) {
  const { tasks } = useDataStore()

  return (
    <div className="space-y-1.5">
      <Label>Task ID</Label>
      {tasks.length > 0 && (
        <Select value={value} onValueChange={onChange}>
          <SelectTrigger className="w-full">
            <SelectValue placeholder="Select a task..." />
          </SelectTrigger>
          <SelectContent>
            {tasks.map((t) => (
              <SelectItem key={t.id} value={t.id}>
                {t.id} ({t.type} / {t.status})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      <Input
        placeholder="Or type a task ID..."
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Level toggle button                                               */
/* ------------------------------------------------------------------ */

const ALL_LEVELS: Level[] = ['debug', 'info', 'warn', 'error']

const levelColors: Record<Level, string> = {
  debug: 'bg-slate-500/20 text-slate-400 border-slate-500/30',
  info: 'bg-blue-500/20 text-blue-400 border-blue-500/30',
  warn: 'bg-amber-500/20 text-amber-400 border-amber-500/30',
  error: 'bg-red-500/20 text-red-400 border-red-500/30',
}

const levelColorsSelected: Record<Level, string> = {
  debug: 'bg-slate-500 text-white border-slate-500',
  info: 'bg-blue-500 text-white border-blue-500',
  warn: 'bg-amber-500 text-white border-amber-500',
  error: 'bg-red-500 text-white border-red-500',
}

function LevelToggle({
  selected,
  onChange,
}: {
  selected: Level[]
  onChange: (levels: Level[]) => void
}) {
  const toggle = (level: Level) => {
    if (selected.includes(level)) {
      onChange(selected.filter((l) => l !== level))
    } else {
      onChange([...selected, level])
    }
  }

  return (
    <div className="space-y-1.5">
      <Label>Filter Levels</Label>
      <div className="flex flex-wrap gap-1.5">
        {ALL_LEVELS.map((level) => {
          const isSelected = selected.includes(level)
          return (
            <button
              key={level}
              type="button"
              onClick={() => toggle(level)}
              className={`rounded-full border px-2.5 py-0.5 text-xs font-medium transition-colors ${
                isSelected ? levelColorsSelected[level] : levelColors[level]
              }`}
            >
              {level}
            </button>
          )
        })}
      </div>
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Status badge                                                      */
/* ------------------------------------------------------------------ */

type SubStatus = 'idle' | 'connecting' | 'connected' | 'done' | 'error'

function statusBadgeVariant(status: SubStatus): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (status) {
    case 'connected': return 'default'
    case 'connecting': return 'secondary'
    case 'done': return 'outline'
    case 'error': return 'destructive'
    case 'idle': return 'outline'
  }
}

function StatusIndicator({
  status,
  doneReason,
  error,
  eventCount,
}: {
  status: SubStatus
  doneReason: string
  error: string | null
  eventCount: number
}) {
  return (
    <div className="flex items-center gap-2 border-b px-3 py-1.5">
      <Badge variant={statusBadgeVariant(status)} className="text-[10px]">
        {status}
      </Badge>
      {status === 'done' && doneReason && (
        <span className="text-[10px] text-muted-foreground">({doneReason})</span>
      )}
      {status === 'error' && error && (
        <span className="truncate text-[10px] text-destructive">{error}</span>
      )}
      <span className="ml-auto text-[10px] text-muted-foreground">
        {eventCount} event{eventCount !== 1 ? 's' : ''}
      </span>
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Single event row                                                  */
/* ------------------------------------------------------------------ */

function EventRow({ envelope }: { envelope: SSEEnvelope }) {
  const [expanded, setExpanded] = useState(false)

  const time = new Date(envelope.timestamp).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  })

  return (
    <div className="border-b px-3 py-1.5 text-xs last:border-b-0">
      <div
        className="flex cursor-pointer items-center gap-2"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
          {time}
        </span>
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
          #{envelope.filteredIndex}
        </span>
        <Badge variant="outline" className="shrink-0 text-[10px] px-1.5 py-0">
          {envelope.type}
        </Badge>
        <span
          className={`shrink-0 rounded-full px-1.5 py-0 text-[10px] font-medium ${
            levelColorsSelected[envelope.level]
          }`}
        >
          {envelope.level}
        </span>
        {envelope.seriesId && (
          <span className="shrink-0 text-[10px] text-muted-foreground">
            [series: {envelope.seriesId}]
          </span>
        )}
        <span className="ml-auto text-[10px] text-muted-foreground">
          {expanded ? '\u25BC' : '\u25B6'}
        </span>
      </div>
      {expanded && (
        <pre className="mt-1 max-h-40 overflow-auto rounded bg-muted/50 p-2 text-[11px] whitespace-pre-wrap break-all">
          {JSON.stringify(envelope.data, null, 2)}
        </pre>
      )}
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Accumulated text display                                          */
/* ------------------------------------------------------------------ */

function AccumulatedTextDisplay({ events }: { events: SSEEnvelope[] }) {
  const accumulatedSeries = new Map<string, string>()

  for (const e of events) {
    if (e.seriesMode === 'accumulate' && e.seriesId) {
      const current = accumulatedSeries.get(e.seriesId) ?? ''
      const field = e.seriesAccField ?? 'text'
      const chunk =
        e.data && typeof e.data === 'object' && field in (e.data as Record<string, unknown>)
          ? String((e.data as Record<string, unknown>)[field])
          : typeof e.data === 'string'
            ? e.data
            : ''
      accumulatedSeries.set(e.seriesId, current + chunk)
    }
  }

  if (accumulatedSeries.size === 0) return null

  return (
    <div className="border-t">
      <div className="px-3 py-1.5 text-[10px] font-medium text-muted-foreground">
        Accumulated Text
      </div>
      {[...accumulatedSeries.entries()].map(([seriesId, text]) => (
        <div key={seriesId} className="border-t px-3 py-2">
          <span className="text-[10px] text-muted-foreground">Series: {seriesId}</span>
          <pre className="mt-1 max-h-32 overflow-auto rounded bg-muted/50 p-2 text-xs whitespace-pre-wrap break-words">
            {text}
          </pre>
        </div>
      ))}
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Main panel                                                        */
/* ------------------------------------------------------------------ */

export function BrowserPanel({ panel }: { panel: Panel }) {
  const { removePanel } = usePanelStore()
  const { addEvent } = useDataStore()
  const { baseUrl, effectiveToken } = useApi(panel)

  // Config state
  const [taskId, setTaskId] = useState('')
  const [filterTypes, setFilterTypes] = useState('')
  const [filterLevels, setFilterLevels] = useState<Level[]>([])

  // Subscription state
  const [events, setEvents] = useState<SSEEnvelope[]>([])
  const [status, setStatus] = useState<SubStatus>('idle')
  const [doneReason, setDoneReason] = useState('')
  const [error, setError] = useState<string | null>(null)

  // Abort controller ref for cancellation
  const abortRef = useRef<AbortController | null>(null)

  // Auto-scroll ref
  const scrollEndRef = useRef<HTMLDivElement | null>(null)
  const [autoScroll, setAutoScroll] = useState(true)

  useEffect(() => {
    if (autoScroll && scrollEndRef.current) {
      scrollEndRef.current.scrollIntoView({ behavior: 'smooth' })
    }
  }, [events.length, autoScroll])

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (abortRef.current) {
        abortRef.current.abort()
      }
    }
  }, [])

  const handleSubscribe = useCallback(async () => {
    if (!taskId.trim()) {
      setError('Task ID is required')
      return
    }

    // Abort previous subscription
    if (abortRef.current) {
      abortRef.current.abort()
    }

    const controller = new AbortController()
    abortRef.current = controller

    setStatus('connecting')
    setEvents([])
    setDoneReason('')
    setError(null)

    let receivedFirst = false

    // Build filter
    const types = filterTypes
      .split(',')
      .map((t) => t.trim())
      .filter(Boolean)
    const filter =
      types.length > 0 || filterLevels.length > 0
        ? {
            ...(types.length > 0 ? { types } : {}),
            ...(filterLevels.length > 0 ? { levels: filterLevels } : {}),
          }
        : undefined

    // Create client with custom fetch that passes the abort signal
    const client = new TaskcastClient({
      baseUrl,
      token: effectiveToken,
      fetch: (input, init) =>
        globalThis.fetch(input, { ...init, signal: controller.signal }),
    })

    try {
      await client.subscribe(taskId.trim(), {
        filter,
        onEvent: (envelope) => {
          if (!receivedFirst) {
            receivedFirst = true
            setStatus('connected')
          }
          setEvents((prev) => [...prev, envelope])
          // Also add to global store as TaskEvent-like structure
          addEvent({
            id: envelope.eventId,
            taskId: envelope.taskId,
            index: envelope.rawIndex,
            timestamp: envelope.timestamp,
            type: envelope.type,
            level: envelope.level,
            data: envelope.data,
            seriesId: envelope.seriesId,
            seriesMode: envelope.seriesMode,
            seriesAccField: envelope.seriesAccField,
          })
        },
        onDone: (reason) => {
          setStatus('done')
          setDoneReason(reason)
          abortRef.current = null
        },
        onError: (err) => {
          setStatus('error')
          setError(err.message)
          abortRef.current = null
        },
      })

      // subscribe() promise resolves when the stream ends normally
      // If status hasn't been set to 'done' or 'error' already, mark as done
      setStatus((prev) => (prev === 'connecting' || prev === 'connected' ? 'done' : prev))
    } catch (e) {
      // AbortError means the user cancelled
      if ((e as Error).name === 'AbortError') {
        setStatus('idle')
      } else {
        setStatus('error')
        setError((e as Error).message)
      }
    } finally {
      abortRef.current = null
    }
  }, [taskId, filterTypes, filterLevels, baseUrl, effectiveToken, addEvent])

  const handleUnsubscribe = useCallback(() => {
    if (abortRef.current) {
      abortRef.current.abort()
      abortRef.current = null
    }
    setStatus('idle')
  }, [])

  const isSubscribing = status === 'connecting' || status === 'connected'
  const configDisabled = isSubscribing

  return (
    <div className="flex h-full flex-col">
      {/* Panel header */}
      <div className="flex items-center justify-between border-b px-3 py-2">
        <span className="text-sm font-medium">{panel.label}</span>
        <div className="flex items-center gap-1">
          <PanelAuthConfig panel={panel} />
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={() => {
              handleUnsubscribe()
              removePanel(panel.id)
            }}
          >
            &times;
          </Button>
        </div>
      </div>

      {/* Config area */}
      <div className="space-y-3 border-b p-3">
        <div className={configDisabled ? 'pointer-events-none opacity-60' : ''}>
          <TaskIdField value={taskId} onChange={setTaskId} />
        </div>

        <div className={configDisabled ? 'pointer-events-none opacity-60' : ''}>
          <div className="space-y-1.5">
            <Label>Filter Types (comma-separated)</Label>
            <Input
              value={filterTypes}
              onChange={(e) => setFilterTypes(e.target.value)}
              placeholder="llm.*, system.*"
              disabled={configDisabled}
            />
          </div>
        </div>

        <div className={configDisabled ? 'pointer-events-none opacity-60' : ''}>
          <LevelToggle selected={filterLevels} onChange={setFilterLevels} />
        </div>

        <div className="flex items-center gap-2">
          {!isSubscribing ? (
            <Button onClick={handleSubscribe} size="sm" disabled={!taskId.trim()}>
              Subscribe
            </Button>
          ) : (
            <Button onClick={handleUnsubscribe} size="sm" variant="destructive">
              Unsubscribe
            </Button>
          )}
          {events.length > 0 && (
            <Button
              onClick={() => setEvents([])}
              size="sm"
              variant="outline"
              disabled={isSubscribing}
            >
              Clear
            </Button>
          )}
          <label className="ml-auto flex cursor-pointer items-center gap-1.5 text-[10px] text-muted-foreground">
            <input
              type="checkbox"
              checked={autoScroll}
              onChange={(e) => setAutoScroll(e.target.checked)}
              className="h-3 w-3"
            />
            Auto-scroll
          </label>
        </div>
      </div>

      {/* Status indicator */}
      <StatusIndicator
        status={status}
        doneReason={doneReason}
        error={error}
        eventCount={events.length}
      />

      {/* Event stream */}
      <ScrollArea className="flex-1">
        <div className="min-h-0">
          {events.length === 0 && status === 'idle' && (
            <div className="flex items-center justify-center p-8 text-sm text-muted-foreground">
              Configure and subscribe to see SSE events
            </div>
          )}
          {events.length === 0 && status === 'connecting' && (
            <div className="flex items-center justify-center p-8 text-sm text-muted-foreground">
              Connecting...
            </div>
          )}
          {events.map((envelope, i) => (
            <EventRow key={`${envelope.eventId}-${i}`} envelope={envelope} />
          ))}
          <div ref={scrollEndRef} />
        </div>

        {/* Accumulated text display */}
        <AccumulatedTextDisplay events={events} />
      </ScrollArea>
    </div>
  )
}
