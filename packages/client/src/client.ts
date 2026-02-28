import { createParser } from 'eventsource-parser'
import type { SSEEnvelope, SubscribeFilter } from '@taskcast/core'

export interface SubscribeOptions {
  filter?: SubscribeFilter
  onEvent: (envelope: SSEEnvelope) => void
  onDone: (reason: string) => void
  onError?: (err: Error) => void
}

export interface TaskcastClientOptions {
  baseUrl: string
  token?: string
  fetch?: typeof globalThis.fetch
}

export class TaskcastClient {
  private baseUrl: string
  private token?: string
  private fetch: typeof globalThis.fetch

  constructor(opts: TaskcastClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/$/, '')
    if (opts.token !== undefined) this.token = opts.token
    this.fetch = opts.fetch ?? globalThis.fetch
  }

  async subscribe(taskId: string, opts: SubscribeOptions): Promise<void> {
    const url = this._buildURL(taskId, opts.filter)
    const headers: Record<string, string> = { Accept: 'text/event-stream' }
    if (this.token) headers['Authorization'] = `Bearer ${this.token}`

    const res = await this.fetch(url, { headers })
    if (!res.ok) {
      throw new Error(`Failed to subscribe: HTTP ${res.status}`)
    }
    if (!res.body) throw new Error('No response body')

    const reader = res.body.getReader()
    const decoder = new TextDecoder()

    const parser = createParser((parseEvent) => {
      if (parseEvent.type !== 'event') return
      if (parseEvent.event === 'taskcast.event') {
        try {
          const envelope = JSON.parse(parseEvent.data) as SSEEnvelope
          opts.onEvent(envelope)
        } catch {
          // ignore parse errors
        }
      } else if (parseEvent.event === 'taskcast.done') {
        try {
          const { reason } = JSON.parse(parseEvent.data) as { reason: string }
          opts.onDone(reason)
        } catch {
          // ignore parse errors
        }
      }
    })

    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      parser.feed(decoder.decode(value, { stream: true }))
    }
  }

  private _buildURL(taskId: string, filter?: SubscribeFilter): string {
    const params = new URLSearchParams()
    if (filter?.types) params.set('types', filter.types.join(','))
    if (filter?.levels) params.set('levels', filter.levels.join(','))
    if (filter?.includeStatus === false) params.set('includeStatus', 'false')
    if (filter?.wrap === false) params.set('wrap', 'false')
    if (filter?.since?.id) params.set('since.id', filter.since.id)
    if (filter?.since?.index !== undefined) params.set('since.index', String(filter.since.index))
    if (filter?.since?.timestamp !== undefined)
      params.set('since.timestamp', String(filter.since.timestamp))

    const qs = params.toString()
    return `${this.baseUrl}/tasks/${taskId}/events${qs ? `?${qs}` : ''}`
  }
}
