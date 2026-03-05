import { useState, useCallback } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { Badge } from '@/components/ui/badge'
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

/* ------------------------------------------------------------------ */
/*  Shared: response display                                          */
/* ------------------------------------------------------------------ */

interface ApiResponse {
  status: number
  body: unknown
}

function statusBadgeVariant(code: number): 'default' | 'secondary' | 'destructive' | 'outline' {
  if (code >= 200 && code < 300) return 'default'
  if (code >= 400 && code < 500) return 'secondary'
  if (code >= 500) return 'destructive'
  return 'outline'
}

function ResponseDisplay({ response }: { response: ApiResponse | null }) {
  if (!response) return null
  return (
    <div className="mt-3 space-y-2">
      <Badge variant={statusBadgeVariant(response.status)}>{response.status}</Badge>
      <ScrollArea className="max-h-48 rounded border">
        <pre className="p-2 text-xs whitespace-pre-wrap break-all">
          {JSON.stringify(response.body, null, 2)}
        </pre>
      </ScrollArea>
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  Shared: task ID selector (select from known tasks or type custom) */
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
/*  Helper: safe JSON parse                                           */
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

/* ------------------------------------------------------------------ */
/*  Tab 1: Create Task                                                */
/* ------------------------------------------------------------------ */

function CreateTaskTab({ panel }: { panel: Panel }) {
  const { apiFetch } = useApi(panel)
  const { addTask } = useDataStore()

  const [type, setType] = useState('llm.chat')
  const [params, setParams] = useState('{}')
  const [ttl, setTtl] = useState('')
  const [tags, setTags] = useState('')
  const [assignMode, setAssignMode] = useState('external')
  const [cost, setCost] = useState('')

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [response, setResponse] = useState<ApiResponse | null>(null)

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
  }, [apiFetch, addTask, type, params, ttl, tags, assignMode, cost])

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
    if (seriesMode) body.seriesMode = seriesMode

    setLoading(true)
    try {
      const res = await apiFetch(`/tasks/${taskId.trim()}/events`, {
        method: 'POST',
        body: JSON.stringify(body),
      })
      const json = await res.json()
      setResponse({ status: res.status, body: json })
      if (res.status === 201) addEvent(json)
    } catch (e) {
      setResponse({ status: 0, body: { error: (e as Error).message } })
    } finally {
      setLoading(false)
    }
  }, [apiFetch, addEvent, taskId, type, level, data, seriesId, seriesMode])

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
        <Button
          variant="ghost"
          size="icon-xs"
          onClick={() => removePanel(panel.id)}
        >
          &times;
        </Button>
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
