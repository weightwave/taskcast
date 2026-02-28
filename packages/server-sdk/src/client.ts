import type { Task, TaskEvent, TaskStatus, TaskAuthConfig, WebhookConfig, CleanupRule, SeriesMode } from '@taskcast/core'

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
    return this._request<Task>('POST', '/tasks', input, 201)
  }

  async getTask(taskId: string): Promise<Task> {
    return this._request<Task>('GET', `/tasks/${taskId}`)
  }

  async transitionTask(
    taskId: string,
    status: TaskStatus,
    payload?: { result?: Task['result']; error?: Task['error'] },
  ): Promise<Task> {
    return this._request<Task>('PATCH', `/tasks/${taskId}/status`, {
      status,
      ...payload,
    })
  }

  async publishEvent(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    return this._request<TaskEvent>('POST', `/tasks/${taskId}/events`, input, 201)
  }

  async publishEvents(taskId: string, inputs: PublishEventInput[]): Promise<TaskEvent[]> {
    return this._request<TaskEvent[]>('POST', `/tasks/${taskId}/events`, inputs, 201)
  }

  async getHistory(
    taskId: string,
    opts?: { since?: { id?: string; index?: number; timestamp?: number } },
  ): Promise<TaskEvent[]> {
    const params = new URLSearchParams()
    if (opts?.since?.id) params.set('since.id', opts.since.id)
    if (opts?.since?.index !== undefined) params.set('since.index', String(opts.since.index))
    if (opts?.since?.timestamp !== undefined)
      params.set('since.timestamp', String(opts.since.timestamp))
    const qs = params.toString()
    return this._request<TaskEvent[]>('GET', `/tasks/${taskId}/events/history${qs ? `?${qs}` : ''}`)
  }

  private async _request<T>(
    method: string,
    path: string,
    body?: unknown,
    expectedStatus = 200,
  ): Promise<T> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      Accept: 'application/json',
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
