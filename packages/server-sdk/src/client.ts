import type { Task, TaskEvent, TaskStatus, TaskAuthConfig, WebhookConfig, CleanupRule, SeriesMode, SinceCursor, TaskError, SubscribeFilter } from '@taskcast/core'

export type CreateTaskInput = Pick<Partial<Task>, 'type' | 'params' | 'result' | 'metadata' | 'ttl' | 'authConfig' | 'webhooks' | 'cleanup'>

export interface PublishEventInput {
  type: string
  level: 'debug' | 'info' | 'warn' | 'error'
  data: unknown
  seriesId?: string
  seriesMode?: SeriesMode
}

export interface TaskcastServerClientOptions {
  baseUrl: string
  token?: string
  fetch?: typeof globalThis.fetch
}

export class TaskcastServerClient {
  private fetch: typeof globalThis.fetch
  private baseUrl: string
  private token?: string

  constructor(opts: TaskcastServerClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/$/, '')
    if (opts.token !== undefined) {
      this.token = opts.token
    }
    this.fetch = opts.fetch ?? globalThis.fetch
  }

  async createTask(input: CreateTaskInput): Promise<Task> {
    return this._request<Task>('POST', '/tasks', input)
  }

  async getTask(taskId: string): Promise<Task> {
    return this._request<Task>('GET', `/tasks/${taskId}`)
  }

  async transitionTask(
    taskId: string,
    status: TaskStatus,
    payload?: { result?: Record<string, unknown>; error?: TaskError },
  ): Promise<Task> {
    return this._request<Task>('PATCH', `/tasks/${taskId}/status`, {
      status,
      ...payload,
    })
  }

  async publishEvent(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    return this._request<TaskEvent>('POST', `/tasks/${taskId}/events`, input)
  }

  async publishEvents(taskId: string, inputs: PublishEventInput[]): Promise<TaskEvent[]> {
    return this._request<TaskEvent[]>('POST', `/tasks/${taskId}/events`, inputs)
  }

  async getHistory(
    taskId: string,
    opts?: { since?: SinceCursor },
  ): Promise<TaskEvent[]> {
    const params = new URLSearchParams()
    if (opts?.since?.id) params.set('since.id', opts.since.id)
    if (opts?.since?.index !== undefined) params.set('since.index', String(opts.since.index))
    if (opts?.since?.timestamp !== undefined)
      params.set('since.timestamp', String(opts.since.timestamp))
    const qs = params.toString()
    return this._request<TaskEvent[]>('GET', `/tasks/${taskId}/events/history${qs ? `?${qs}` : ''}`)
  }

  /**
   * Subscribe to real-time events for a task via SSE.
   * Connects to the server's SSE endpoint and calls the handler for each event.
   * Returns a synchronous unsubscribe function that closes the connection.
   *
   * Always uses `wrap=false` so the handler receives raw `TaskEvent` objects.
   * The connection starts asynchronously; events begin flowing once established.
   * The stream closes automatically when the task reaches a terminal status.
   */
  subscribe(
    taskId: string,
    handler: (event: TaskEvent) => void,
    filter?: Omit<SubscribeFilter, 'wrap'>,
  ): () => void {
    const controller = new AbortController()

    const params = new URLSearchParams()
    // Force unwrapped TaskEvent format to match the handler type
    params.set('wrap', 'false')
    if (filter?.types?.length) params.set('types', filter.types.join(','))
    if (filter?.levels?.length) params.set('levels', filter.levels.join(','))
    if (filter?.includeStatus !== undefined) params.set('includeStatus', String(filter.includeStatus))
    if (filter?.seriesFormat) params.set('seriesFormat', filter.seriesFormat)
    if (filter?.since?.id) params.set('since.id', filter.since.id)
    if (filter?.since?.index !== undefined) params.set('since.index', String(filter.since.index))
    if (filter?.since?.timestamp !== undefined) params.set('since.timestamp', String(filter.since.timestamp))

    const qs = params.toString()
    const url = `${this.baseUrl}/tasks/${taskId}/events${qs ? `?${qs}` : ''}`
    const headers: Record<string, string> = { Accept: 'text/event-stream' }
    if (this.token) headers['Authorization'] = `Bearer ${this.token}`

    // Start SSE connection asynchronously
    void this._consumeSSE(url, headers, controller.signal, handler)

    return () => controller.abort()
  }

  private async _consumeSSE(
    url: string,
    headers: Record<string, string>,
    signal: AbortSignal,
    handler: (event: TaskEvent) => void,
  ): Promise<void> {
    let res: Response
    try {
      res = await this.fetch(url, { method: 'GET', headers, signal })
    } catch {
      return // Aborted or network error
    }
    if (!res.ok || !res.body) return

    const reader = res.body.getReader()
    const decoder = new TextDecoder()
    let buffer = ''
    let currentEvent = ''
    let currentData = ''

    try {
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break

        buffer += decoder.decode(value, { stream: true })
        const lines = buffer.split('\n')
        // Keep incomplete last line in buffer
        buffer = lines.pop() ?? ''

        for (const line of lines) {
          if (line.startsWith('event: ')) {
            currentEvent = line.slice(7).trim()
          } else if (line.startsWith('data: ')) {
            currentData += (currentData ? '\n' : '') + line.slice(6)
          } else if (line === '') {
            // Empty line = end of SSE message
            if (currentEvent === 'taskcast.done') {
              // Terminal event — proactively close the stream
              currentEvent = ''
              currentData = ''
              await reader.cancel().catch(() => {})
              return
            }
            if (currentData) {
              let parsed: TaskEvent | null = null
              try {
                parsed = JSON.parse(currentData) as TaskEvent
              } catch {
                // Skip malformed events
              }
              if (parsed) {
                try {
                  handler(parsed)
                } catch {
                  // Don't let handler errors break the stream
                }
              }
            }
            currentEvent = ''
            currentData = ''
          }
        }
      }
    } catch (err) {
      // AbortError — clean exit; let other errors propagate
      const isAbort =
        (err instanceof DOMException && err.name === 'AbortError') ||
        (err instanceof Error && err.name === 'AbortError')
      if (!isAbort) throw err
    }
  }

  private async _request<T>(
    method: string,
    path: string,
    body?: unknown,
  ): Promise<T> {
    const headers: Record<string, string> = {
      Accept: 'application/json',
    }
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json'
    }
    if (this.token) headers['Authorization'] = `Bearer ${this.token}`

    const init: RequestInit = { method, headers }
    if (body !== undefined) {
      init.body = JSON.stringify(body)
    }
    const res = await this.fetch(`${this.baseUrl}${path}`, init)

    if (!res.ok) {
      let message = `HTTP ${res.status}`
      try {
        const err = await res.json()
        message = (err as { error?: string }).error ?? message
      } catch {
        // ignore parse errors
      }
      throw new Error(message)
    }

    return res.json() as Promise<T>
  }
}
