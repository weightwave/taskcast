import { useState, useCallback } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
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
import { tryParseJson, ResponseDisplay, TaskIdField } from '@/components/shared/utils'
import type { ApiResponse } from '@/components/shared/utils'

/* ------------------------------------------------------------------ */
/*  Tab 1: Create Task                                                */
/* ------------------------------------------------------------------ */

interface WebhookEntry {
  url: string
  secret: string
  filterTypes: string
  filterLevels: string
}

function emptyWebhook(): WebhookEntry {
  return { url: '', secret: '', filterTypes: '', filterLevels: '' }
}

function CreateTaskTab({ panel }: { panel: Panel }) {
  const { apiFetch } = useApi(panel)
  const { addTask } = useDataStore()

  const [type, setType] = useState('llm.chat')
  const [params, setParams] = useState('{}')
  const [ttl, setTtl] = useState('')
  const [tags, setTags] = useState('')
  const [assignMode, setAssignMode] = useState('external')
  const [cost, setCost] = useState('')
  const [webhooks, setWebhooks] = useState<WebhookEntry[]>([])

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [response, setResponse] = useState<ApiResponse | null>(null)

  const updateWebhook = useCallback((idx: number, patch: Partial<WebhookEntry>) => {
    setWebhooks((prev) => prev.map((w, i) => (i === idx ? { ...w, ...patch } : w)))
  }, [])

  const removeWebhook = useCallback((idx: number) => {
    setWebhooks((prev) => prev.filter((_, i) => i !== idx))
  }, [])

  const handleSubmit = useCallback(async () => {
    setError('')
    const parsed = tryParseJson(params)
    if (!parsed.ok) {
      setError(`Invalid params JSON: ${parsed.error}`)
      return
    }

    const body: Record<string, unknown> = { type }
    if (parsed.value !== undefined) body.params = parsed.value
    if (ttl) body.ttl = Number(ttl)
    if (tags.trim()) body.tags = tags.split(',').map((t) => t.trim()).filter(Boolean)
    if (assignMode) body.assignMode = assignMode
    if (cost) body.cost = Number(cost)

    const validWebhooks = webhooks
      .filter((w) => w.url.trim())
      .map((w) => {
        const wh: Record<string, unknown> = { url: w.url.trim() }
        if (w.secret.trim()) wh.secret = w.secret.trim()
        const types = w.filterTypes.split(',').map((t) => t.trim()).filter(Boolean)
        const levels = w.filterLevels.split(',').map((l) => l.trim()).filter(Boolean)
        if (types.length > 0 || levels.length > 0) {
          const filter: Record<string, unknown> = {}
          if (types.length > 0) filter.types = types
          if (levels.length > 0) filter.levels = levels
          wh.filter = filter
        }
        return wh
      })
    if (validWebhooks.length > 0) body.webhooks = validWebhooks

    setLoading(true)
    try {
      const res = await apiFetch('/tasks', {
        method: 'POST',
        body: JSON.stringify(body),
      })
      const json = await res.json()
      setResponse({ status: res.status, body: json })
      if (res.status === 201) addTask(json)
    } catch (e) {
      setResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setLoading(false)
    }
  }, [apiFetch, addTask, type, params, ttl, tags, assignMode, cost, webhooks])

  return (
    <div className="space-y-3 p-3">
      <div className="space-y-1.5">
        <Label>Type</Label>
        <Input value={type} onChange={(e) => setType(e.target.value)} placeholder="llm.chat" />
      </div>
      <div className="space-y-1.5">
        <Label>Params (JSON)</Label>
        <Textarea
          value={params}
          onChange={(e) => setParams(e.target.value)}
          placeholder='{"model": "gpt-4"}'
          rows={3}
        />
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-1.5">
          <Label>TTL (seconds)</Label>
          <Input type="number" value={ttl} onChange={(e) => setTtl(e.target.value)} placeholder="300" />
        </div>
        <div className="space-y-1.5">
          <Label>Cost</Label>
          <Input type="number" value={cost} onChange={(e) => setCost(e.target.value)} placeholder="0" />
        </div>
      </div>
      <div className="space-y-1.5">
        <Label>Tags (comma-separated)</Label>
        <Input value={tags} onChange={(e) => setTags(e.target.value)} placeholder="urgent, batch-1" />
      </div>
      <div className="space-y-1.5">
        <Label>Assign Mode</Label>
        <Select value={assignMode} onValueChange={setAssignMode}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="external">external</SelectItem>
            <SelectItem value="pull">pull</SelectItem>
            <SelectItem value="ws-offer">ws-offer</SelectItem>
            <SelectItem value="ws-race">ws-race</SelectItem>
          </SelectContent>
        </Select>
      </div>

      {/* Webhooks */}
      <div className="space-y-1.5">
        <div className="flex items-center justify-between">
          <Label>Webhooks</Label>
          <Button
            variant="outline"
            size="sm"
            className="h-6 text-[10px] px-2"
            onClick={() => setWebhooks((prev) => [...prev, emptyWebhook()])}
          >
            + Add
          </Button>
        </div>
        {webhooks.map((wh, idx) => (
          <div key={idx} className="space-y-1.5 rounded border p-2">
            <div className="flex items-center gap-1.5">
              <Input
                value={wh.url}
                onChange={(e) => updateWebhook(idx, { url: e.target.value })}
                placeholder="https://example.com/webhook"
                className="text-xs"
              />
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={() => removeWebhook(idx)}
              >
                &times;
              </Button>
            </div>
            <Input
              value={wh.secret}
              onChange={(e) => updateWebhook(idx, { secret: e.target.value })}
              placeholder="HMAC secret (optional)"
              className="text-xs"
            />
            <div className="grid grid-cols-2 gap-1.5">
              <Input
                value={wh.filterTypes}
                onChange={(e) => updateWebhook(idx, { filterTypes: e.target.value })}
                placeholder="Filter types (e.g. llm.*)"
                className="text-xs"
              />
              <Input
                value={wh.filterLevels}
                onChange={(e) => updateWebhook(idx, { filterLevels: e.target.value })}
                placeholder="Filter levels (e.g. info,error)"
                className="text-xs"
              />
            </div>
          </div>
        ))}
      </div>

      {error && <p className="text-xs text-destructive">{error}</p>}

      <Button onClick={handleSubmit} disabled={loading} size="sm">
        {loading ? 'Creating...' : 'Create Task'}
      </Button>

      <ResponseDisplay response={response} />
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Tab 2: Transition Status                                          */
/* ------------------------------------------------------------------ */

function TransitionStatusTab({ panel }: { panel: Panel }) {
  const { apiFetch } = useApi(panel)
  const { updateTask } = useDataStore()

  const [taskId, setTaskId] = useState('')
  const [status, setStatus] = useState('running')
  const [result, setResult] = useState('')

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [response, setResponse] = useState<ApiResponse | null>(null)

  const handleSubmit = useCallback(async () => {
    setError('')
    if (!taskId.trim()) {
      setError('Task ID is required')
      return
    }

    const parsedResult = tryParseJson(result)
    if (!parsedResult.ok) {
      setError(`Invalid result JSON: ${parsedResult.error}`)
      return
    }

    const body: Record<string, unknown> = { status }
    if (parsedResult.value !== undefined) body.result = parsedResult.value

    setLoading(true)
    try {
      const res = await apiFetch(`/tasks/${taskId.trim()}/status`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      })
      const json = await res.json()
      setResponse({ status: res.status, body: json })
      if (res.ok) updateTask(json)
    } catch (e) {
      setResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setLoading(false)
    }
  }, [apiFetch, updateTask, taskId, status, result])

  return (
    <div className="space-y-3 p-3">
      <TaskIdField value={taskId} onChange={setTaskId} />

      <div className="space-y-1.5">
        <Label>Target Status</Label>
        <Select value={status} onValueChange={setStatus}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {['running', 'completed', 'failed', 'cancelled', 'timeout', 'paused', 'blocked', 'assigned'].map((s) => (
              <SelectItem key={s} value={s}>
                {s}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="space-y-1.5">
        <Label>Result (JSON, optional)</Label>
        <Textarea
          value={result}
          onChange={(e) => setResult(e.target.value)}
          placeholder='{"output": "..."}'
          rows={3}
        />
      </div>

      {error && <p className="text-xs text-destructive">{error}</p>}

      <Button onClick={handleSubmit} disabled={loading} size="sm">
        {loading ? 'Transitioning...' : 'Transition Status'}
      </Button>

      <ResponseDisplay response={response} />
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Tab 3: Publish Event                                              */
/* ------------------------------------------------------------------ */

function PublishEventTab({ panel }: { panel: Panel }) {
  const { apiFetch } = useApi(panel)
  const { addEvent } = useDataStore()

  const [taskId, setTaskId] = useState('')
  const [type, setType] = useState('llm.token')
  const [level, setLevel] = useState('info')
  const [data, setData] = useState('{}')
  const [seriesId, setSeriesId] = useState('')
  const [seriesMode, setSeriesMode] = useState('')

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [response, setResponse] = useState<ApiResponse | null>(null)

  const handleSubmit = useCallback(async () => {
    setError('')
    if (!taskId.trim()) {
      setError('Task ID is required')
      return
    }

    const parsedData = tryParseJson(data)
    if (!parsedData.ok) {
      setError(`Invalid data JSON: ${parsedData.error}`)
      return
    }

    const body: Record<string, unknown> = { type, level }
    if (parsedData.value !== undefined) body.data = parsedData.value
    if (seriesId.trim()) body.seriesId = seriesId.trim()
    if (seriesMode && seriesMode !== 'none') body.seriesMode = seriesMode

    setLoading(true)
    try {
      const res = await apiFetch(`/tasks/${taskId.trim()}/events`, {
        method: 'POST',
        body: JSON.stringify(body),
      })
      const json = await res.json()
      setResponse({ status: res.status, body: json })
      if (res.status === 201) addEvent(json, 'sent', panel.label)
    } catch (e) {
      setResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setLoading(false)
    }
  }, [apiFetch, addEvent, panel.label, taskId, type, level, data, seriesId, seriesMode])

  return (
    <div className="space-y-3 p-3">
      <TaskIdField value={taskId} onChange={setTaskId} />

      <div className="space-y-1.5">
        <Label>Event Type</Label>
        <Input value={type} onChange={(e) => setType(e.target.value)} placeholder="llm.token" />
      </div>

      <div className="space-y-1.5">
        <Label>Level</Label>
        <Select value={level} onValueChange={setLevel}>
          <SelectTrigger className="w-full">
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

      <div className="space-y-1.5">
        <Label>Data (JSON)</Label>
        <Textarea
          value={data}
          onChange={(e) => setData(e.target.value)}
          placeholder='{"token": "hello"}'
          rows={3}
        />
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-1.5">
          <Label>Series ID</Label>
          <Input
            value={seriesId}
            onChange={(e) => setSeriesId(e.target.value)}
            placeholder="optional"
          />
        </div>
        <div className="space-y-1.5">
          <Label>Series Mode</Label>
          <Select value={seriesMode} onValueChange={setSeriesMode}>
            <SelectTrigger className="w-full">
              <SelectValue placeholder="none" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">none</SelectItem>
              <SelectItem value="keep-all">keep-all</SelectItem>
              <SelectItem value="accumulate">accumulate</SelectItem>
              <SelectItem value="latest">latest</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      {error && <p className="text-xs text-destructive">{error}</p>}

      <Button onClick={handleSubmit} disabled={loading} size="sm">
        {loading ? 'Publishing...' : 'Publish Event'}
      </Button>

      <ResponseDisplay response={response} />
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Tab 4: Query                                                      */
/* ------------------------------------------------------------------ */

function QueryTab({ panel }: { panel: Panel }) {
  const { apiFetch } = useApi(panel)

  const [taskId, setTaskId] = useState('')
  const [action, setAction] = useState('get-task')

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [response, setResponse] = useState<ApiResponse | null>(null)

  const handleSubmit = useCallback(async () => {
    setError('')
    if (!taskId.trim()) {
      setError('Task ID is required')
      return
    }

    const path =
      action === 'get-task'
        ? `/tasks/${taskId.trim()}`
        : `/tasks/${taskId.trim()}/events/history`

    setLoading(true)
    try {
      const res = await apiFetch(path, { method: 'GET' })
      const json = await res.json()
      setResponse({ status: res.status, body: json })
    } catch (e) {
      setResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setLoading(false)
    }
  }, [apiFetch, taskId, action])

  return (
    <div className="space-y-3 p-3">
      <TaskIdField value={taskId} onChange={setTaskId} />

      <div className="space-y-1.5">
        <Label>Action</Label>
        <Select value={action} onValueChange={setAction}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="get-task">Get Task</SelectItem>
            <SelectItem value="get-events">Get Events</SelectItem>
          </SelectContent>
        </Select>
      </div>

      {error && <p className="text-xs text-destructive">{error}</p>}

      <Button onClick={handleSubmit} disabled={loading} size="sm">
        {loading ? 'Loading...' : action === 'get-task' ? 'Get Task' : 'Get Events'}
      </Button>

      <ResponseDisplay response={response} />
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Main panel                                                        */
/* ------------------------------------------------------------------ */

export function BackendPanel({ panel }: { panel: Panel }) {
  const { removePanel } = usePanelStore()

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b px-3 py-2">
        <span className="text-sm font-medium">{panel.label}</span>
        <div className="flex items-center gap-1">
          <PanelAuthConfig panel={panel} />
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={() => removePanel(panel.id)}
          >
            &times;
          </Button>
        </div>
      </div>

      <ScrollArea className="flex-1">
        <Tabs defaultValue="create" className="w-full">
          <TabsList className="mx-3 mt-2 w-[calc(100%-1.5rem)]">
            <TabsTrigger value="create">Create</TabsTrigger>
            <TabsTrigger value="transition">Status</TabsTrigger>
            <TabsTrigger value="event">Event</TabsTrigger>
            <TabsTrigger value="query">Query</TabsTrigger>
          </TabsList>

          <TabsContent value="create">
            <CreateTaskTab panel={panel} />
          </TabsContent>

          <TabsContent value="transition">
            <TransitionStatusTab panel={panel} />
          </TabsContent>

          <TabsContent value="event">
            <PublishEventTab panel={panel} />
          </TabsContent>

          <TabsContent value="query">
            <QueryTab panel={panel} />
          </TabsContent>
        </Tabs>
      </ScrollArea>
    </div>
  )
}
