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
import { usePanelStore } from '@/stores'
import type { Panel } from '@/stores'
import { useApi } from '@/hooks/useApi'
import { useConnectionStore } from '@/stores'
import { PanelAuthConfig } from '@/components/panels/PanelAuthConfig'

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

type WorkerStatus = 'idle' | 'registering' | 'polling' | 'assigned' | 'error'

interface ClaimedTask {
  id: string
  type?: string
  status?: string
  params?: Record<string, unknown>
  tags?: string[]
  cost?: number
  assignMode?: string
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

function statusBadgeVariant(status: WorkerStatus): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (status) {
    case 'polling': return 'default'
    case 'assigned': return 'default'
    case 'registering': return 'secondary'
    case 'error': return 'destructive'
    case 'idle': return 'outline'
  }
}

let pullWorkerCounter = 0

/* ------------------------------------------------------------------ */
/*  Response display                                                   */
/* ------------------------------------------------------------------ */

function ResponseDisplay({ response }: { response: ApiResponse | null }) {
  if (!response) return null
  const variant = response.status >= 200 && response.status < 300 ? 'default'
    : response.status >= 400 && response.status < 500 ? 'secondary'
    : response.status >= 500 ? 'destructive'
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
/*  Task processing section                                            */
/* ------------------------------------------------------------------ */

function TaskProcessing({
  task,
  panel,
  onTaskDone,
}: {
  task: ClaimedTask
  panel: Panel
  onTaskDone: () => void
}) {
  const { apiFetch } = useApi(panel)
  const [mode, setMode] = useState<'manual' | 'auto'>('manual')

  // Manual mode state
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

  // Auto mode state
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
      const res = await apiFetch(`/tasks/${task.id}/status`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      })
      const json = await res.json()
      setTransitionResponse({ status: res.status, body: json })
      if (['completed', 'failed', 'cancelled', 'timeout'].includes(targetStatus) && res.ok) {
        onTaskDone()
      }
    } catch (e) {
      setTransitionResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setTransitionLoading(false)
    }
  }, [apiFetch, task.id, targetStatus, result, onTaskDone])

  const handlePublishEvent = useCallback(async () => {
    const parsedData = tryParseJson(eventData)
    if (!parsedData.ok) return

    const body: Record<string, unknown> = { type: eventType, level: eventLevel }
    if (parsedData.value !== undefined) body.data = parsedData.value
    if (seriesId.trim()) body.seriesId = seriesId.trim()
    if (seriesMode && seriesMode !== 'none') body.seriesMode = seriesMode

    setEventLoading(true)
    try {
      const res = await apiFetch(`/tasks/${task.id}/events`, {
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
  }, [apiFetch, task.id, eventType, eventLevel, eventData, seriesId, seriesMode])

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
      // Step 1: Transition to running
      log('Transitioning to running...')
      const runRes = await apiFetch(`/tasks/${task.id}/status`, {
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

      // Step 2: Publish simulated events
      const eventCount = 3 + Math.floor(Math.random() * 3) // 3-5 events
      for (let i = 0; i < eventCount; i++) {
        await delay(400 + Math.random() * 300)
        log(`Publishing event ${i + 1}/${eventCount}...`)
        const evRes = await apiFetch(`/tasks/${task.id}/events`, {
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

      // Step 3: Transition to completed
      log('Transitioning to completed...')
      const completeRes = await apiFetch(`/tasks/${task.id}/status`, {
        method: 'PATCH',
        body: JSON.stringify({
          status: 'completed',
          result: { output: 'Auto-processed by worker', chunks: eventCount },
        }),
        signal: controller.signal,
      })
      if (completeRes.ok) {
        log('Task completed!')
        onTaskDone()
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
  }, [apiFetch, task.id, onTaskDone])

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      autoAbortRef.current?.abort()
    }
  }, [])

  return (
    <div className="space-y-3">
      <Separator />

      {/* Task details */}
      <div className="space-y-1">
        <div className="text-xs font-medium">Claimed Task</div>
        <div className="rounded border p-2 text-xs space-y-0.5">
          <div><span className="text-muted-foreground">ID:</span> {task.id}</div>
          {task.type && <div><span className="text-muted-foreground">Type:</span> {task.type}</div>}
          {task.status && <div><span className="text-muted-foreground">Status:</span> {task.status}</div>}
          {task.tags && task.tags.length > 0 && (
            <div><span className="text-muted-foreground">Tags:</span> {task.tags.join(', ')}</div>
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
      </div>

      {/* Mode toggle */}
      <div className="flex items-center gap-2">
        <Label className="text-xs">Processing Mode:</Label>
        <Button
          size="sm"
          variant={mode === 'manual' ? 'default' : 'outline'}
          onClick={() => setMode('manual')}
          className="h-6 text-xs"
        >
          Manual
        </Button>
        <Button
          size="sm"
          variant={mode === 'auto' ? 'default' : 'outline'}
          onClick={() => setMode('auto')}
          className="h-6 text-xs"
        >
          Auto
        </Button>
      </div>

      {mode === 'manual' ? (
        <div className="space-y-3">
          {/* Transition */}
          <div className="space-y-1.5">
            <div className="text-xs font-medium">Transition Status</div>
            <div className="flex items-end gap-2">
              <div className="flex-1 space-y-1">
                <Select value={targetStatus} onValueChange={setTargetStatus}>
                  <SelectTrigger className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {['running', 'completed', 'failed', 'cancelled', 'timeout'].map((s) => (
                      <SelectItem key={s} value={s}>{s}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <Button size="sm" onClick={handleTransition} disabled={transitionLoading}>
                {transitionLoading ? 'Sending...' : 'Send'}
              </Button>
            </div>
            <div className="space-y-1">
              <Label className="text-xs">Result (JSON, optional)</Label>
              <Textarea
                value={result}
                onChange={(e) => setResult(e.target.value)}
                placeholder='{"output": "..."}'
                rows={2}
              />
            </div>
            <ResponseDisplay response={transitionResponse} />
          </div>

          <Separator />

          {/* Publish Event */}
          <div className="space-y-1.5">
            <div className="text-xs font-medium">Publish Event</div>
            <div className="space-y-1.5">
              <Input
                value={eventType}
                onChange={(e) => setEventType(e.target.value)}
                placeholder="Event type"
              />
              <Select value={eventLevel} onValueChange={setEventLevel}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {['debug', 'info', 'warn', 'error'].map((l) => (
                    <SelectItem key={l} value={l}>{l}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Textarea
                value={eventData}
                onChange={(e) => setEventData(e.target.value)}
                placeholder='{"token": "hello"}'
                rows={2}
              />
              <div className="grid grid-cols-2 gap-2">
                <Input
                  value={seriesId}
                  onChange={(e) => setSeriesId(e.target.value)}
                  placeholder="Series ID"
                />
                <Select value={seriesMode} onValueChange={setSeriesMode}>
                  <SelectTrigger className="w-full">
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
              <Button size="sm" onClick={handlePublishEvent} disabled={eventLoading}>
                {eventLoading ? 'Publishing...' : 'Publish Event'}
              </Button>
              <ResponseDisplay response={eventResponse} />
            </div>
          </div>
        </div>
      ) : (
        <div className="space-y-2">
          <Button
            size="sm"
            onClick={handleAutoProcess}
            disabled={autoRunning}
          >
            {autoRunning ? 'Processing...' : 'Auto Process'}
          </Button>
          {autoRunning && (
            <Button
              size="sm"
              variant="destructive"
              onClick={() => autoAbortRef.current?.abort()}
            >
              Cancel
            </Button>
          )}
          {autoLog.length > 0 && (
            <ScrollArea className="max-h-40 rounded border">
              <div className="p-2 text-xs space-y-0.5">
                {autoLog.map((msg, i) => (
                  <div key={i} className="text-muted-foreground">{msg}</div>
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

export function WorkerPullPanel({ panel }: { panel: Panel }) {
  const { removePanel } = usePanelStore()
  const { apiFetch, baseUrl, effectiveToken } = useApi(panel)
  const { mode: connectionMode } = useConnectionStore()

  // Config state
  const [workerId, setWorkerId] = useState(() => `worker-pull-${++pullWorkerCounter}`)
  const [matchTypes, setMatchTypes] = useState('llm.*')
  const [weight, setWeight] = useState('1')
  const [timeout, setTimeout_] = useState('30000')

  // Worker state
  const [status, setStatus] = useState<WorkerStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [claimedTask, setClaimedTask] = useState<ClaimedTask | null>(null)
  const [pollCount, setPollCount] = useState(0)

  // Refs for cleanup
  const wsRef = useRef<WebSocket | null>(null)
  const pollAbortRef = useRef<AbortController | null>(null)
  const pollingRef = useRef(false)

  /* ---------------------------------------------------------------- */
  /*  Build WS URL for registration                                    */
  /* ---------------------------------------------------------------- */

  const getWsUrl = useCallback(() => {
    if (connectionMode === 'embedded' || baseUrl === '/taskcast') {
      return `ws://${window.location.host}/workers/ws`
    }
    // External mode: derive from baseUrl
    // baseUrl might be like http://localhost:3721/taskcast
    const httpUrl = baseUrl.replace('/taskcast', '')
    return httpUrl.replace(/^http/, 'ws') + '/workers/ws'
  }, [connectionMode, baseUrl])

  /* ---------------------------------------------------------------- */
  /*  Build worker fetch URL for pull endpoint                         */
  /* ---------------------------------------------------------------- */

  const getWorkerFetchUrl = useCallback(
    (params: Record<string, string>) => {
      const qs = new URLSearchParams(params).toString()
      if (connectionMode === 'embedded' || baseUrl === '/taskcast') {
        return `/workers/pull?${qs}`
      }
      const httpUrl = baseUrl.replace('/taskcast', '')
      return `${httpUrl}/workers/pull?${qs}`
    },
    [connectionMode, baseUrl],
  )

  /* ---------------------------------------------------------------- */
  /*  Build auth headers for direct worker calls                       */
  /* ---------------------------------------------------------------- */

  const getHeaders = useCallback((): Record<string, string> => {
    const h: Record<string, string> = { 'Content-Type': 'application/json' }
    if (effectiveToken) h['Authorization'] = `Bearer ${effectiveToken}`
    return h
  }, [effectiveToken])

  /* ---------------------------------------------------------------- */
  /*  Poll loop                                                        */
  /* ---------------------------------------------------------------- */

  const startPollLoop = useCallback(async () => {
    pollingRef.current = true
    const controller = new AbortController()
    pollAbortRef.current = controller

    const typesList = matchTypes
      .split(',')
      .map((t) => t.trim())
      .filter(Boolean)

    setStatus('polling')
    setPollCount(0)

    while (pollingRef.current && !controller.signal.aborted) {
      try {
        const params: Record<string, string> = {
          workerId,
          timeout: timeout || '30000',
        }
        if (weight && weight !== '1') params.weight = weight

        const url = getWorkerFetchUrl(params)
        const res = await fetch(url, {
          signal: controller.signal,
          headers: getHeaders(),
        })

        if (controller.signal.aborted) break

        if (res.status === 200) {
          // Task claimed
          const task = await res.json() as ClaimedTask
          setClaimedTask(task)
          setStatus('assigned')
          pollingRef.current = false
          break
        } else if (res.status === 204) {
          // No task available, continue polling
          setPollCount((prev) => prev + 1)
          continue
        } else {
          // Error
          const body = await res.json().catch(() => ({ error: res.statusText }))
          setError(`Poll error ${res.status}: ${JSON.stringify(body)}`)
          setStatus('error')
          pollingRef.current = false
          break
        }
      } catch (e) {
        if ((e as Error).name === 'AbortError') break
        // Wait and retry on network error
        setError(`Network error: ${(e as Error).message}`)
        try {
          await new Promise<void>((resolve, reject) => {
            const timer = window.setTimeout(resolve, 2000)
            controller.signal.addEventListener('abort', () => {
              clearTimeout(timer)
              reject(new Error('aborted'))
            })
          })
        } catch {
          break
        }
      }
    }
  }, [workerId, matchTypes, weight, timeout, getWorkerFetchUrl, getHeaders])

  /* ---------------------------------------------------------------- */
  /*  Start: register via WS, then start polling                       */
  /* ---------------------------------------------------------------- */

  const handleStart = useCallback(() => {
    setError(null)
    setClaimedTask(null)
    setStatus('registering')

    const typesList = matchTypes
      .split(',')
      .map((t) => t.trim())
      .filter(Boolean)

    const wsUrl = getWsUrl()
    const ws = new WebSocket(wsUrl)
    wsRef.current = ws

    ws.onopen = () => {
      // Register with the server
      ws.send(
        JSON.stringify({
          type: 'register',
          workerId,
          matchRule: { taskTypes: typesList },
          capacity: 10,
          weight: Number(weight) || 1,
        }),
      )
    }

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data) as Record<string, unknown>
        if (msg.type === 'registered') {
          // Registration successful, start polling
          startPollLoop()
        } else if (msg.type === 'ping') {
          // Respond to pings to keep the registration alive
          ws.send(JSON.stringify({ type: 'pong' }))
        } else if (msg.type === 'error') {
          setError(`WS error: ${msg.message as string}`)
          setStatus('error')
        }
        // Ignore offer/available messages - we use HTTP polling instead
      } catch {
        // Ignore parse errors
      }
    }

    ws.onerror = () => {
      setError('WebSocket connection failed. Is the server running with workers enabled?')
      setStatus('error')
    }

    ws.onclose = () => {
      // If we're still polling, the WS closed unexpectedly
      if (pollingRef.current) {
        setError('Registration WebSocket closed unexpectedly')
        setStatus('error')
        pollingRef.current = false
        pollAbortRef.current?.abort()
      }
    }
  }, [workerId, matchTypes, weight, getWsUrl, startPollLoop])

  /* ---------------------------------------------------------------- */
  /*  Stop polling                                                     */
  /* ---------------------------------------------------------------- */

  const handleStop = useCallback(() => {
    pollingRef.current = false
    pollAbortRef.current?.abort()
    pollAbortRef.current = null
    if (wsRef.current) {
      wsRef.current.close()
      wsRef.current = null
    }
    setStatus('idle')
    setClaimedTask(null)
  }, [])

  /* ---------------------------------------------------------------- */
  /*  Task done callback                                               */
  /* ---------------------------------------------------------------- */

  const handleTaskDone = useCallback(() => {
    setClaimedTask(null)
    // Resume polling if the WS is still open
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      startPollLoop()
    } else {
      setStatus('idle')
    }
  }, [startPollLoop])

  /* ---------------------------------------------------------------- */
  /*  Cleanup on unmount                                               */
  /* ---------------------------------------------------------------- */

  useEffect(() => {
    return () => {
      pollingRef.current = false
      pollAbortRef.current?.abort()
      if (wsRef.current) {
        wsRef.current.close()
        wsRef.current = null
      }
    }
  }, [])

  const isActive = status === 'registering' || status === 'polling' || status === 'assigned'
  const configDisabled = isActive

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
              handleStop()
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
                <Label className="text-xs">Worker ID</Label>
                <Input
                  value={workerId}
                  onChange={(e) => setWorkerId(e.target.value)}
                  placeholder="worker-pull-1"
                />
              </div>
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
                  <Label className="text-xs">Weight</Label>
                  <Input
                    type="number"
                    value={weight}
                    onChange={(e) => setWeight(e.target.value)}
                    placeholder="1"
                  />
                </div>
                <div className="space-y-1">
                  <Label className="text-xs">Timeout (ms)</Label>
                  <Input
                    type="number"
                    value={timeout}
                    onChange={(e) => setTimeout_(e.target.value)}
                    placeholder="30000"
                  />
                </div>
              </div>
            </div>
          </div>

          {/* Controls */}
          <div className="flex items-center gap-2">
            {!isActive ? (
              <Button onClick={handleStart} size="sm" disabled={!workerId.trim()}>
                Start Polling
              </Button>
            ) : (
              <Button onClick={handleStop} size="sm" variant="destructive">
                Stop Polling
              </Button>
            )}
          </div>

          {/* Status display */}
          <div className="flex items-center gap-2">
            <Badge variant={statusBadgeVariant(status)} className="text-[10px]">
              {status}
            </Badge>
            {status === 'polling' && (
              <span className="text-[10px] text-muted-foreground">
                Poll cycles: {pollCount}
              </span>
            )}
            {error && (
              <span className="text-[10px] text-destructive truncate">{error}</span>
            )}
          </div>

          {/* Current task */}
          {claimedTask && (
            <TaskProcessing
              task={claimedTask}
              panel={panel}
              onTaskDone={handleTaskDone}
            />
          )}

          {/* Idle / polling hint */}
          {status === 'idle' && !claimedTask && (
            <div className="text-xs text-muted-foreground text-center py-4">
              Configure and start polling to receive tasks.
              <br />
              <span className="text-[10px]">
                Tasks must have assignMode &quot;pull&quot; to be claimed by this worker.
              </span>
            </div>
          )}

          {status === 'polling' && !claimedTask && (
            <div className="text-xs text-muted-foreground text-center py-4">
              Waiting for a matching task...
            </div>
          )}
        </div>
      </ScrollArea>
    </div>
  )
}
