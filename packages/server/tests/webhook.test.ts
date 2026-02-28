import { describe, it, expect, vi } from 'vitest'
import { WebhookDelivery } from '../src/webhook.js'
import type { TaskEvent, WebhookConfig } from '@taskcast/core'

const makeEvent = (): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 1700000000000,
  type: 'llm.delta',
  level: 'info',
  data: { text: 'hello' },
})

describe('WebhookDelivery', () => {
  it('sends POST request to webhook URL', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledOnce()
    const [url, opts] = fetch.mock.calls[0]!
    expect(url).toBe('https://example.com/hook')
    expect(opts.method).toBe('POST')
    expect(opts.headers['Content-Type']).toBe('application/json')
    expect(opts.headers['X-Taskcast-Event']).toBe('llm.delta')
    expect(opts.headers['X-Taskcast-Timestamp']).toBeTruthy()
  })

  it('includes HMAC signature when secret is configured', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      secret: 'test-secret',
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    const [, opts] = fetch.mock.calls[0]!
    expect(opts.headers['X-Taskcast-Signature']).toMatch(/^sha256=/)
  })

  it('retries on non-2xx response', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 3, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledTimes(2)
  })

  it('throws after exhausting retries', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response('error', { status: 500 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 2, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    expect(fetch).toHaveBeenCalledTimes(3)
  })

  it('uses linear backoff strategy', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 3, backoff: 'linear', initialDelayMs: 0, maxDelayMs: 1000, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledTimes(2)
  })

  it('skips delivery when event does not match filter', async () => {
    const fetch = vi.fn()
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      filter: { types: ['other.event'] },
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    // makeEvent() returns type 'llm.delta', which doesn't match 'other.event'
    await delivery.send(makeEvent(), config)
    expect(fetch).not.toHaveBeenCalled()
  })

  it('handles fetch throwing an error (network error) with retry', async () => {
    const fetch = vi
      .fn()
      .mockRejectedValueOnce(new Error('Network failure'))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 3, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledTimes(2)
  })
})
