import { useState, useCallback, useRef, useEffect } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { usePanelStore, useConnectionStore } from '@/stores'
import type { Panel } from '@/stores'
import { useApi } from '@/hooks/useApi'
import { PanelAuthConfig } from '@/components/panels/PanelAuthConfig'

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'registered'

interface WsLogEntry {
  id: number
  timestamp: number
  direction: 'sent' | 'received'
  msgType: string
  payload: unknown
}

interface TaskOffer {
  taskId: string
  task: {
    id: string
    type?: string
    tags?: string[]
    cost?: number
    params?: Record<string, unknown>
  }
  offerType: 'offer' | 'available'
}

interface AssignedTask {
  taskId: string
  type?: string
  tags?: string[]
  cost?: number
  params?: Record<string, unknown>
}

interface ApiResponse {
  status: number
  body: unknown
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function tryParseJson(text: string): { ok: true; value: unknown } | { ok: false; error: string } {
  const trimmed = text.trim()
  if (trimmed === '') return { ok: true, value: undefined }
  try {
    return { ok: true, value: JSON.parse(trimmed) }
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
}

function statusBadgeVariant(
  status: ConnectionStatus,
): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (status) {
    case 'registered':
      return 'default'
    case 'connected':
      return 'secondary'
    case 'connecting':
      return 'secondary'
    case 'disconnected':
      return 'outline'
  }
}

let logIdCounter = 0

/* ------------------------------------------------------------------ */
/*  Response display                                                   */
/* ------------------------------------------------------------------ */

function ResponseDisplay({ response }: { response: ApiResponse | null }) {
  if (!response) return null
  const variant =
    response.status >= 200 && response.status < 300
      ? 'default'
      : response.status >= 400 && response.status < 500
        ? 'secondary'
        : response.status >= 500
          ? 'destructive'
          : 'outline'
  return (
    <div className="mt-2 space-y-1">
      <Badge variant={variant}>{response.status}</Badge>
      <ScrollArea className="max-h-32 rounded border">
        <pre className="p-2 text-xs whitespace-pre-wrap break-all">
          {JSON.stringify(response.body, null, 2)}
        </pre>
      </ScrollArea>
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Message log entry component                                        */
/* ------------------------------------------------------------------ */

function LogEntry({ entry }: { entry: WsLogEntry }) {
  const [expanded, setExpanded] = useState(false)

  const time = new Date(entry.timestamp).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  })

  const directionClass =
    entry.direction === 'sent' ? 'bg-blue-500/10' : 'bg-green-500/10'
  const arrow = entry.direction === 'sent' ? '\u2192' : '\u2190'
  const arrowClass =
    entry.direction === 'sent' ? 'text-blue-400' : 'text-green-400'

  return (
    <div
      className={`border-b px-2 py-1 text-xs last:border-b-0 ${directionClass} cursor-pointer`}
      onClick={() => setExpanded(!expanded)}
    >
      <div className="flex items-center gap-1.5">
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
          {time}
        </span>
        <span className={`shrink-0 font-bold text-[10px] ${arrowClass}`}>{arrow}</span>
        <Badge variant="outline" className="shrink-0 text-[10px] px-1 py-0">
          {entry.msgType}
        </Badge>
        {!expanded && (
          <span className="truncate text-[10px] text-muted-foreground">
            {typeof entry.payload === 'string'
              ? entry.payload
              : JSON.stringify(entry.payload).slice(0, 60)}
          </span>
        )}
      </div>
      {expanded && (
        <pre className="mt-1 max-h-24 overflow-auto rounded bg-muted/50 p-1.5 text-[10px] whitespace-pre-wrap break-all">
          {JSON.stringify(entry.payload, null, 2)}
        </pre>
      )}
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Assigned task processing (same as pull panel)                      */
/* ------------------------------------------------------------------ */

function AssignedTaskProcessing({
  task,
  panel,
  onTaskDone,
}: {
  task: AssignedTask
  panel: Panel
  onTaskDone: (taskId: string) => void
}) {
  const { apiFetch } = useApi(panel)
  const [mode, setMode] = useState<'manual' | 'auto'>('manual')

  // Manual mode
  const [targetStatus, setTargetStatus] = useState('running')
  const [result, setResult] = useState('')
  const [transitionResponse, setTransitionResponse] = useState<ApiResponse | null>(null)
  const [transitionLoading, setTransitionLoading] = useState(false)

  const [eventType, setEventType] = useState('llm.token')
  const [eventLevel, setEventLevel] = useState('info')
  const [eventData, setEventData] = useState('{}')
  const [seriesId, setSeriesId] = useState('')
  const [seriesMode, setSeriesMode] = useState('')
  const [eventResponse, setEventResponse] = useState<ApiResponse | null>(null)
  const [eventLoading, setEventLoading] = useState(false)

  // Auto mode
  const [autoRunning, setAutoRunning] = useState(false)
  const [autoLog, setAutoLog] = useState<string[]>([])
  const autoAbortRef = useRef<AbortController | null>(null)

  const handleTransition = useCallback(async () => {
    const parsedResult = tryParseJson(result)
    if (!parsedResult.ok) return

    const body: Record<string, unknown> = { status: targetStatus }
    if (parsedResult.value !== undefined) body.result = parsedResult.value

    setTransitionLoading(true)
    try {
      const res = await apiFetch(`/tasks/${task.taskId}/status`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      })
      const json = await res.json()
      setTransitionResponse({ status: res.status, body: json })
      if (['completed', 'failed', 'cancelled', 'timeout'].includes(targetStatus) && res.ok) {
        onTaskDone(task.taskId)
      }
    } catch (e) {
      setTransitionResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setTransitionLoading(false)
    }
  }, [apiFetch, task.taskId, targetStatus, result, onTaskDone])

  const handlePublishEvent = useCallback(async () => {
    const parsedData = tryParseJson(eventData)
    if (!parsedData.ok) return

    const body: Record<string, unknown> = { type: eventType, level: eventLevel }
    if (parsedData.value !== undefined) body.data = parsedData.value
    if (seriesId.trim()) body.seriesId = seriesId.trim()
    if (seriesMode && seriesMode !== 'none') body.seriesMode = seriesMode

    setEventLoading(true)
    try {
      const res = await apiFetch(`/tasks/${task.taskId}/events`, {
        method: 'POST',
        body: JSON.stringify(body),
      })
      const json = await res.json()
      setEventResponse({ status: res.status, body: json })
    } catch (e) {
      setEventResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setEventLoading(false)
    }
  }, [apiFetch, task.taskId, eventType, eventLevel, eventData, seriesId, seriesMode])

  const handleAutoProcess = useCallback(async () => {
    const controller = new AbortController()
    autoAbortRef.current = controller
    setAutoRunning(true)
    setAutoLog([])

    const log = (msg: string) => setAutoLog((prev) => [...prev, msg])
    const delay = (ms: number) =>
      new Promise<void>((resolve, reject) => {
        const timer = setTimeout(resolve, ms)
        controller.signal.addEventListener('abort', () => {
          clearTimeout(timer)
          reject(new Error('aborted'))
        })
      })

    try {
      log('Transitioning to running...')
      const runRes = await apiFetch(`/tasks/${task.taskId}/status`, {
        method: 'PATCH',
        body: JSON.stringify({ status: 'running' }),
        signal: controller.signal,
      })
      if (!runRes.ok) {
        const err = await runRes.json()
        log(`Failed to transition: ${JSON.stringify(err)}`)
        return
      }
      log('Status: running')

      const eventCount = 3 + Math.floor(Math.random() * 3)
      for (let i = 0; i < eventCount; i++) {
        await delay(400 + Math.random() * 300)
        log(`Publishing event ${i + 1}/${eventCount}...`)
        const evRes = await apiFetch(`/tasks/${task.taskId}/events`, {
          method: 'POST',
          body: JSON.stringify({
            type: 'llm.token',
            level: 'info',
            data: { token: `chunk-${i + 1}`, index: i },
            seriesId: 'output',
            seriesMode: 'accumulate',
          }),
          signal: controller.signal,
        })
        if (evRes.ok) {
          log(`Event ${i + 1} published`)
        } else {
          const err = await evRes.json()
          log(`Event ${i + 1} failed: ${JSON.stringify(err)}`)
        }
      }

      await delay(300)

      log('Transitioning to completed...')
      const completeRes = await apiFetch(`/tasks/${task.taskId}/status`, {
        method: 'PATCH',
        body: JSON.stringify({
          status: 'completed',
          result: { output: 'Auto-processed by WS worker', chunks: eventCount },
        }),
        signal: controller.signal,
      })
      if (completeRes.ok) {
        log('Task completed!')
        onTaskDone(task.taskId)
      } else {
        const err = await completeRes.json()
        log(`Failed to complete: ${JSON.stringify(err)}`)
      }
    } catch (e) {
      if ((e as Error).message !== 'aborted') {
        log(`Error: ${(e as Error).message}`)
      } else {
        log('Auto-process aborted')
      }
    } finally {
      setAutoRunning(false)
      autoAbortRef.current = null
    }
  }, [apiFetch, task.taskId, onTaskDone])

  useEffect(() => {
    return () => {
      autoAbortRef.current?.abort()
    }
  }, [])

  return (
    <div className="rounded border p-2 space-y-2">
      <div className="text-xs space-y-0.5">
        <div>
          <span className="text-muted-foreground">ID:</span> {task.taskId}
        </div>
        {task.type && (
          <div>
            <span className="text-muted-foreground">Type:</span> {task.type}
          </div>
        )}
        {task.tags && task.tags.length > 0 && (
          <div>
            <span className="text-muted-foreground">Tags:</span> {task.tags.join(', ')}
          </div>
        )}
        {task.params && (
          <div>
            <span className="text-muted-foreground">Params:</span>
            <pre className="mt-0.5 text-[10px] whitespace-pre-wrap break-all">
              {JSON.stringify(task.params, null, 2)}
            </pre>
          </div>
        )}
      </div>

      <div className="flex items-center gap-2">
        <Label className="text-xs">Mode:</Label>
        <Button
          size="sm"
          variant={mode === 'manual' ? 'default' : 'outline'}
          onClick={() => setMode('manual')}
          className="h-5 text-[10px] px-2"
        >
          Manual
        </Button>
        <Button
          size="sm"
          variant={mode === 'auto' ? 'default' : 'outline'}
          onClick={() => setMode('auto')}
          className="h-5 text-[10px] px-2"
        >
          Auto
        </Button>
      </div>

      {mode === 'manual' ? (
        <div className="space-y-2">
          {/* Transition */}
          <div className="flex items-end gap-1.5">
            <div className="flex-1">
              <Select value={targetStatus} onValueChange={setTargetStatus}>
                <SelectTrigger className="h-7 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {['running', 'completed', 'failed', 'cancelled', 'timeout'].map((s) => (
                    <SelectItem key={s} value={s}>
                      {s}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <Button size="sm" className="h-7 text-xs" onClick={handleTransition} disabled={transitionLoading}>
              {transitionLoading ? '...' : 'Transition'}
            </Button>
          </div>
          <Textarea
            value={result}
            onChange={(e) => setResult(e.target.value)}
            placeholder='Result JSON (optional)'
            rows={1}
            className="text-xs"
          />
          <ResponseDisplay response={transitionResponse} />

          <Separator />

          {/* Publish event */}
          <div className="space-y-1.5">
            <div className="grid grid-cols-2 gap-1.5">
              <Input
                value={eventType}
                onChange={(e) => setEventType(e.target.value)}
                placeholder="Event type"
                className="h-7 text-xs"
              />
              <Select value={eventLevel} onValueChange={setEventLevel}>
                <SelectTrigger className="h-7 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {['debug', 'info', 'warn', 'error'].map((l) => (
                    <SelectItem key={l} value={l}>
                      {l}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <Textarea
              value={eventData}
              onChange={(e) => setEventData(e.target.value)}
              placeholder='{"token": "hello"}'
              rows={1}
              className="text-xs"
            />
            <div className="grid grid-cols-2 gap-1.5">
              <Input
                value={seriesId}
                onChange={(e) => setSeriesId(e.target.value)}
                placeholder="Series ID"
                className="h-7 text-xs"
              />
              <Select value={seriesMode} onValueChange={setSeriesMode}>
                <SelectTrigger className="h-7 text-xs">
                  <SelectValue placeholder="Series mode" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">none</SelectItem>
                  <SelectItem value="keep-all">keep-all</SelectItem>
                  <SelectItem value="accumulate">accumulate</SelectItem>
                  <SelectItem value="latest">latest</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <Button size="sm" className="h-7 text-xs" onClick={handlePublishEvent} disabled={eventLoading}>
              {eventLoading ? '...' : 'Publish Event'}
            </Button>
            <ResponseDisplay response={eventResponse} />
          </div>
        </div>
      ) : (
        <div className="space-y-1.5">
          <div className="flex items-center gap-2">
            <Button
              size="sm"
              className="h-7 text-xs"
              onClick={handleAutoProcess}
              disabled={autoRunning}
            >
              {autoRunning ? 'Processing...' : 'Auto Process'}
            </Button>
            {autoRunning && (
              <Button
                size="sm"
                variant="destructive"
                className="h-7 text-xs"
                onClick={() => autoAbortRef.current?.abort()}
              >
                Cancel
              </Button>
            )}
          </div>
          {autoLog.length > 0 && (
            <ScrollArea className="max-h-24 rounded border">
              <div className="p-1.5 text-[10px] space-y-0.5">
                {autoLog.map((msg, i) => (
                  <div key={i} className="text-muted-foreground">
                    {msg}
                  </div>
                ))}
              </div>
            </ScrollArea>
          )}
        </div>
      )}
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Main panel                                                         */
/* ------------------------------------------------------------------ */

export function WorkerWsPanel({ panel }: { panel: Panel }) {
  const { removePanel } = usePanelStore()
  const { baseUrl, effectiveToken } = useApi(panel)
  const { mode: connectionMode } = useConnectionStore()

  // Config state
  const [matchTypes, setMatchTypes] = useState('llm.*')
  const [capacity, setCapacity] = useState('5')
  const [weight, setWeight] = useState('1')

  // Connection state
  const [status, setStatus] = useState<ConnectionStatus>('disconnected')
  const [workerId, setWorkerId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  // Message log
  const [log, setLog] = useState<WsLogEntry[]>([])
  const logEndRef = useRef<HTMLDivElement | null>(null)

  // Task offers and assigned tasks
  const [offers, setOffers] = useState<TaskOffer[]>([])
  const [assignedTasks, setAssignedTasks] = useState<AssignedTask[]>([])

  // WS ref
  const wsRef = useRef<WebSocket | null>(null)

  /* ---------------------------------------------------------------- */
  /*  Auto-scroll log                                                  */
  /* ---------------------------------------------------------------- */

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [log.length])

  /* ---------------------------------------------------------------- */
  /*  Build WS URL                                                     */
  /* ---------------------------------------------------------------- */

  const getWsUrl = useCallback(() => {
    if (connectionMode === 'embedded' || baseUrl === '/taskcast') {
      return `ws://${window.location.host}/workers/ws`
    }
    const httpUrl = baseUrl.replace('/taskcast', '')
    return httpUrl.replace(/^http/, 'ws') + '/workers/ws'
  }, [connectionMode, baseUrl])

  /* ---------------------------------------------------------------- */
  /*  Log helpers                                                      */
  /* ---------------------------------------------------------------- */

  const addLog = useCallback(
    (direction: 'sent' | 'received', msgType: string, payload: unknown) => {
      setLog((prev) => [
        ...prev.slice(-199), // Keep last 200 entries
        {
          id: ++logIdCounter,
          timestamp: Date.now(),
          direction,
          msgType,
          payload,
        },
      ])
    },
    [],
  )

  /* ---------------------------------------------------------------- */
  /*  Send message helper                                              */
  /* ---------------------------------------------------------------- */

  const sendMessage = useCallback(
    (msg: Record<string, unknown>) => {
      if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
        const json = JSON.stringify(msg)
        wsRef.current.send(json)
        addLog('sent', msg.type as string, msg)
      }
    },
    [addLog],
  )

  /* ---------------------------------------------------------------- */
  /*  Connect                                                          */
  /* ---------------------------------------------------------------- */

  const handleConnect = useCallback(() => {
    setError(null)
    setStatus('connecting')
    setWorkerId(null)
    setOffers([])
    setAssignedTasks([])

    const typesList = matchTypes
      .split(',')
      .map((t) => t.trim())
      .filter(Boolean)

    const wsUrl = getWsUrl()
    const ws = new WebSocket(wsUrl)
    wsRef.current = ws

    ws.onopen = () => {
      setStatus('connected')
      // Send register message
      const registerMsg = {
        type: 'register',
        matchRule: { taskTypes: typesList },
        capacity: Number(capacity) || 5,
        weight: Number(weight) || 1,
      }
      const json = JSON.stringify(registerMsg)
      ws.send(json)
      addLog('sent', 'register', registerMsg)
    }

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data as string) as Record<string, unknown>
        const msgType = (msg.type as string) || 'unknown'
        addLog('received', msgType, msg)

        switch (msgType) {
          case 'registered':
            setStatus('registered')
            setWorkerId(msg.workerId as string)
            break

          case 'ping':
            // Auto-respond with pong
            sendMessage({ type: 'pong' })
            break

          case 'offer': {
            const offer: TaskOffer = {
              taskId: msg.taskId as string,
              task: msg.task as TaskOffer['task'],
              offerType: 'offer',
            }
            setOffers((prev) => [...prev, offer])
            break
          }

          case 'available': {
            const available: TaskOffer = {
              taskId: msg.taskId as string,
              task: msg.task as TaskOffer['task'],
              offerType: 'available',
            }
            setOffers((prev) => [...prev, available])
            break
          }

          case 'assigned': {
            const taskId = msg.taskId as string
            // Move from offers to assigned tasks
            setOffers((prev) => {
              const offer = prev.find((o) => o.taskId === taskId)
              if (offer) {
                setAssignedTasks((at) => [
                  ...at,
                  {
                    taskId,
                    type: offer.task.type,
                    tags: offer.task.tags,
                    cost: offer.task.cost,
                    params: offer.task.params,
                  },
                ])
              } else {
                // Assigned without prior offer (e.g., from claim)
                setAssignedTasks((at) => [...at, { taskId }])
              }
              return prev.filter((o) => o.taskId !== taskId)
            })
            break
          }

          case 'declined':
            setOffers((prev) => prev.filter((o) => o.taskId !== (msg.taskId as string)))
            break

          case 'claimed': {
            const taskId = msg.taskId as string
            if (msg.success) {
              setOffers((prev) => {
                const offer = prev.find((o) => o.taskId === taskId)
                if (offer) {
                  setAssignedTasks((at) => [
                    ...at,
                    {
                      taskId,
                      type: offer.task.type,
                      tags: offer.task.tags,
                      cost: offer.task.cost,
                      params: offer.task.params,
                    },
                  ])
                } else {
                  setAssignedTasks((at) => [...at, { taskId }])
                }
                return prev.filter((o) => o.taskId !== taskId)
              })
            }
            break
          }

          case 'error':
            setError(msg.message as string)
            break
        }
      } catch {
        // Ignore parse errors
      }
    }

    ws.onerror = () => {
      setError('WebSocket connection failed. Is the server running with workers enabled?')
      setStatus('disconnected')
    }

    ws.onclose = () => {
      setStatus('disconnected')
      wsRef.current = null
    }
  }, [matchTypes, capacity, weight, getWsUrl, addLog, sendMessage])

  /* ---------------------------------------------------------------- */
  /*  Disconnect                                                       */
  /* ---------------------------------------------------------------- */

  const handleDisconnect = useCallback(() => {
    if (wsRef.current) {
      wsRef.current.close()
      wsRef.current = null
    }
    setStatus('disconnected')
    setWorkerId(null)
    setOffers([])
    setAssignedTasks([])
  }, [])

  /* ---------------------------------------------------------------- */
  /*  Accept / Decline offer                                           */
  /* ---------------------------------------------------------------- */

  const handleAccept = useCallback(
    (taskId: string) => {
      sendMessage({ type: 'accept', taskId })
    },
    [sendMessage],
  )

  const handleDecline = useCallback(
    (taskId: string) => {
      sendMessage({ type: 'decline', taskId })
    },
    [sendMessage],
  )

  /* ---------------------------------------------------------------- */
  /*  Task done                                                        */
  /* ---------------------------------------------------------------- */

  const handleTaskDone = useCallback((taskId: string) => {
    setAssignedTasks((prev) => prev.filter((t) => t.taskId !== taskId))
  }, [])

  /* ---------------------------------------------------------------- */
  /*  Cleanup on unmount                                               */
  /* ---------------------------------------------------------------- */

  useEffect(() => {
    return () => {
      if (wsRef.current) {
        wsRef.current.close()
        wsRef.current = null
      }
    }
  }, [])

  const isConnected = status === 'connected' || status === 'registered'
  const configDisabled = isConnected

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
              handleDisconnect()
              removePanel(panel.id)
            }}
          >
            &times;
          </Button>
        </div>
      </div>

      <ScrollArea className="flex-1">
        <div className="space-y-3 p-3">
          {/* Config area */}
          <div className={configDisabled ? 'pointer-events-none opacity-60' : ''}>
            <div className="space-y-2">
              <div className="space-y-1">
                <Label className="text-xs">Match Rule Types (comma-separated)</Label>
                <Input
                  value={matchTypes}
                  onChange={(e) => setMatchTypes(e.target.value)}
                  placeholder="llm.*"
                />
              </div>
              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <Label className="text-xs">Capacity</Label>
                  <Input
                    type="number"
                    value={capacity}
                    onChange={(e) => setCapacity(e.target.value)}
                    placeholder="5"
                  />
                </div>
                <div className="space-y-1">
                  <Label className="text-xs">Weight</Label>
                  <Input
                    type="number"
                    value={weight}
                    onChange={(e) => setWeight(e.target.value)}
                    placeholder="1"
                  />
                </div>
              </div>
            </div>
          </div>

          {/* Controls */}
          <div className="flex items-center gap-2">
            {!isConnected ? (
              <Button onClick={handleConnect} size="sm">
                Connect
              </Button>
            ) : (
              <Button onClick={handleDisconnect} size="sm" variant="destructive">
                Disconnect
              </Button>
            )}
          </div>

          {/* Status display */}
          <div className="flex items-center gap-2 flex-wrap">
            <Badge variant={statusBadgeVariant(status)} className="text-[10px]">
              {status}
            </Badge>
            {workerId && (
              <span className="text-[10px] text-muted-foreground">
                Worker: {workerId}
              </span>
            )}
            {error && (
              <span className="text-[10px] text-destructive truncate">{error}</span>
            )}
          </div>

          {/* Task offers */}
          {offers.length > 0 && (
            <div className="space-y-2">
              <div className="text-xs font-medium">
                Task Offers ({offers.length})
              </div>
              {offers.map((offer) => (
                <div
                  key={offer.taskId}
                  className="rounded border p-2 space-y-1.5"
                >
                  <div className="flex items-center gap-2">
                    <Badge
                      variant="outline"
                      className="text-[10px] px-1.5 py-0"
                    >
                      {offer.offerType}
                    </Badge>
                    <span className="text-xs font-mono truncate">
                      {offer.taskId}
                    </span>
                  </div>
                  <div className="text-[10px] text-muted-foreground space-y-0.5">
                    {offer.task.type && <div>Type: {offer.task.type}</div>}
                    {offer.task.tags && (
                      <div>Tags: {offer.task.tags.join(', ')}</div>
                    )}
                    {offer.task.params && (
                      <div>
                        Params: {JSON.stringify(offer.task.params).slice(0, 80)}
                      </div>
                    )}
                  </div>
                  <div className="flex items-center gap-1.5">
                    <Button
                      size="sm"
                      className="h-6 text-[10px] px-2"
                      onClick={() => handleAccept(offer.taskId)}
                    >
                      Accept
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      className="h-6 text-[10px] px-2"
                      onClick={() => handleDecline(offer.taskId)}
                    >
                      Decline
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          )}

          {/* Assigned tasks */}
          {assignedTasks.length > 0 && (
            <div className="space-y-2">
              <div className="text-xs font-medium">
                Assigned Tasks ({assignedTasks.length})
              </div>
              {assignedTasks.map((task) => (
                <AssignedTaskProcessing
                  key={task.taskId}
                  task={task}
                  panel={panel}
                  onTaskDone={handleTaskDone}
                />
              ))}
            </div>
          )}

          {/* Message log */}
          <div className="space-y-1">
            <div className="flex items-center justify-between">
              <div className="text-xs font-medium">
                Message Log ({log.length})
              </div>
              {log.length > 0 && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-5 text-[10px] px-2"
                  onClick={() => setLog([])}
                >
                  Clear
                </Button>
              )}
            </div>
            <div className="max-h-60 overflow-auto rounded border">
              {log.length === 0 ? (
                <div className="p-3 text-center text-[10px] text-muted-foreground">
                  No messages yet
                </div>
              ) : (
                <div>
                  {log.map((entry) => (
                    <LogEntry key={entry.id} entry={entry} />
                  ))}
                  <div ref={logEndRef} />
                </div>
              )}
            </div>
          </div>

          {/* Idle hint */}
          {status === 'disconnected' && (
            <div className="text-xs text-muted-foreground text-center py-2">
              Connect to receive task offers via WebSocket.
              <br />
              <span className="text-[10px]">
                Tasks must have assignMode &quot;ws-offer&quot; or &quot;ws-race&quot;
                to be offered to this worker.
              </span>
            </div>
          )}
        </div>
      </ScrollArea>
    </div>
  )
}
