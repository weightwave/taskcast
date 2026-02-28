import { createHmac } from 'crypto'
import { matchesFilter } from '@taskcast/core'
import type { TaskEvent, WebhookConfig, RetryConfig } from '@taskcast/core'

interface WebhookDeliveryOptions {
  fetch?: typeof globalThis.fetch
}

const DEFAULT_RETRY: RetryConfig = {
  retries: 3,
  backoff: 'exponential',
  initialDelayMs: 1000,
  maxDelayMs: 30000,
  timeoutMs: 5000,
}

export class WebhookDelivery {
  private fetch: typeof globalThis.fetch

  constructor(opts: WebhookDeliveryOptions = {}) {
    this.fetch = opts.fetch ?? globalThis.fetch
  }

  async send(event: TaskEvent, config: WebhookConfig): Promise<void> {
    if (config.filter && !matchesFilter(event, config.filter)) return

    const retry = { ...DEFAULT_RETRY, ...config.retry }
    const body = JSON.stringify(event)
    const timestamp = String(Math.floor(Date.now() / 1000))
    const signature = config.secret ? this._sign(body, config.secret) : undefined

    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      'X-Taskcast-Event': event.type,
      'X-Taskcast-Timestamp': timestamp,
      ...(signature !== undefined ? { 'X-Taskcast-Signature': signature } : {}),
    }

    let lastError: Error | null = null
    for (let attempt = 0; attempt <= retry.retries; attempt++) {
      if (attempt > 0) {
        await this._sleep(this._backoffMs(retry, attempt))
      }
      try {
        const controller = new AbortController()
        const timeout = setTimeout(() => controller.abort(), retry.timeoutMs)
        const res = await this.fetch(config.url, {
          method: 'POST',
          headers,
          body,
          signal: controller.signal,
        })
        clearTimeout(timeout)
        if (res.ok) return
        lastError = new Error(`HTTP ${res.status}`)
      } catch (err) {
        lastError = err instanceof Error ? err : new Error(String(err))
      }
    }

    throw new Error(`Webhook delivery failed after ${retry.retries + 1} attempts: ${lastError?.message}`)
  }

  private _sign(body: string, secret: string): string {
    const hmac = createHmac('sha256', secret)
    hmac.update(body)
    return `sha256=${hmac.digest('hex')}`
  }

  private _backoffMs(retry: RetryConfig, attempt: number): number {
    if (retry.backoff === 'fixed') return retry.initialDelayMs
    if (retry.backoff === 'linear') return retry.initialDelayMs * attempt
    return Math.min(retry.initialDelayMs * Math.pow(2, attempt - 1), retry.maxDelayMs)
  }

  private _sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
  }
}
