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

describe('WebhookDelivery — failure scenarios', () => {
  it('handles fetch timeout (AbortController abort)', async () => {
    // Mock fetch that hangs forever until aborted
    const fetch = vi.fn().mockImplementation(
      (_url: string, opts: { signal: AbortSignal }) =>
        new Promise((_resolve, reject) => {
          opts.signal.addEventListener('abort', () => {
            reject(new DOMException('The operation was aborted', 'AbortError'))
          })
        }),
    )
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 1, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 50 },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    // Should have been called: initial attempt + 1 retry = 2
    expect(fetch).toHaveBeenCalledTimes(2)
  })

  it('handles DNS failure (TypeError: fetch failed)', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://nonexistent.invalid/hook',
      retry: { retries: 2, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    // Initial attempt + 2 retries = 3 calls
    expect(fetch).toHaveBeenCalledTimes(3)
  })

  it('retries with DNS failure then succeeds', async () => {
    const fetch = vi
      .fn()
      .mockRejectedValueOnce(new TypeError('fetch failed'))
      .mockRejectedValueOnce(new TypeError('fetch failed'))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 3, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledTimes(3)
  })

  it('handles non-Error thrown by fetch', async () => {
    const fetch = vi.fn().mockRejectedValue('string error')
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    expect(fetch).toHaveBeenCalledTimes(1)
  })

  it('uses exponential backoff strategy and caps at maxDelayMs', async () => {
    const sleepSpy: number[] = []
    const fetch = vi.fn().mockResolvedValue(new Response('error', { status: 500 }))
    const delivery = new WebhookDelivery({ fetch })

    // Monkey-patch _sleep to capture delays
    const origSleep = (delivery as unknown as { _sleep: (ms: number) => Promise<void> })._sleep
    ;(delivery as unknown as { _sleep: (ms: number) => Promise<void> })._sleep = async (ms: number) => {
      sleepSpy.push(ms)
      // Don't actually sleep
    }

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: {
        retries: 4,
        backoff: 'exponential',
        initialDelayMs: 100,
        maxDelayMs: 500,
        timeoutMs: 5000,
      },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    expect(fetch).toHaveBeenCalledTimes(5) // 1 initial + 4 retries

    // Exponential backoff: 100*2^0=100, 100*2^1=200, 100*2^2=400, 100*2^3=800 -> capped to 500
    expect(sleepSpy).toHaveLength(4)
    expect(sleepSpy[0]).toBe(100)
    expect(sleepSpy[1]).toBe(200)
    expect(sleepSpy[2]).toBe(400)
    expect(sleepSpy[3]).toBe(500) // capped at maxDelayMs
  })

  it('uses linear backoff strategy', async () => {
    const sleepSpy: number[] = []
    const fetch = vi.fn().mockResolvedValue(new Response('error', { status: 500 }))
    const delivery = new WebhookDelivery({ fetch })

    ;(delivery as unknown as { _sleep: (ms: number) => Promise<void> })._sleep = async (ms: number) => {
      sleepSpy.push(ms)
    }

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: {
        retries: 3,
        backoff: 'linear',
        initialDelayMs: 100,
        maxDelayMs: 10000,
        timeoutMs: 5000,
      },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    expect(fetch).toHaveBeenCalledTimes(4) // 1 initial + 3 retries

    // Linear backoff: 100*1=100, 100*2=200, 100*3=300
    expect(sleepSpy).toHaveLength(3)
    expect(sleepSpy[0]).toBe(100)
    expect(sleepSpy[1]).toBe(200)
    expect(sleepSpy[2]).toBe(300)
  })

  it('uses fixed backoff strategy', async () => {
    const sleepSpy: number[] = []
    const fetch = vi.fn().mockResolvedValue(new Response('error', { status: 500 }))
    const delivery = new WebhookDelivery({ fetch })

    ;(delivery as unknown as { _sleep: (ms: number) => Promise<void> })._sleep = async (ms: number) => {
      sleepSpy.push(ms)
    }

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: {
        retries: 3,
        backoff: 'fixed',
        initialDelayMs: 500,
        maxDelayMs: 10000,
        timeoutMs: 5000,
      },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    expect(fetch).toHaveBeenCalledTimes(4) // 1 initial + 3 retries

    // Fixed backoff: always 500
    expect(sleepSpy).toHaveLength(3)
    expect(sleepSpy[0]).toBe(500)
    expect(sleepSpy[1]).toBe(500)
    expect(sleepSpy[2]).toBe(500)
  })

  it('does not include signature header when secret is not configured', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      // No secret configured
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    const [, opts] = fetch.mock.calls[0]!
    expect(opts.headers['X-Taskcast-Signature']).toBeUndefined()
  })

  it('uses default retry config when none provided', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })

    // Monkey-patch _sleep to not actually wait
    ;(delivery as unknown as { _sleep: (ms: number) => Promise<void> })._sleep = async () => {}

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      // No retry config — will use DEFAULT_RETRY (3 retries, exponential)
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledTimes(2)
  })
})
