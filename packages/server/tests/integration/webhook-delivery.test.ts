import { describe, it, expect, vi } from 'vitest'
import { createHmac } from 'crypto'
import { createTestServer } from '../helpers/test-server.js'
import { WebhookDelivery } from '../../src/webhook.js'
import type { WebhookConfig, TaskEvent } from '@taskcast/core'

describe('Server integration — webhook delivery', () => {
  it('engine event triggers webhook delivery with correct payload', async () => {
    const received: { url: string; body: string; headers: Record<string, string> }[] = []
    const mockFetch = vi.fn().mockImplementation(async (url: string, opts: RequestInit) => {
      received.push({
        url,
        body: opts.body as string,
        headers: opts.headers as Record<string, string>,
      })
      return new Response('ok', { status: 200 })
    })

    const delivery = new WebhookDelivery({ fetch: mockFetch })
    const webhookConfig: WebhookConfig = {
      url: 'https://example.com/webhook',
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }

    const { engine } = createTestServer()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')

    // Publish event and deliver via webhook
    const event = (await engine.getEvents(task.id)).find(e => e.type === 'taskcast:status')!
    await delivery.send(event, webhookConfig)

    expect(received).toHaveLength(1)
    const parsed = JSON.parse(received[0]!.body) as TaskEvent
    expect(parsed.taskId).toBe(task.id)
    expect(parsed.type).toBe('taskcast:status')
  })

  it('HMAC signature is valid', async () => {
    const secret = 'webhook-test-secret'
    let receivedSignature = ''
    let receivedBody = ''

    const mockFetch = vi.fn().mockImplementation(async (_url: string, opts: RequestInit) => {
      receivedBody = opts.body as string
      receivedSignature = (opts.headers as Record<string, string>)['X-Taskcast-Signature'] ?? ''
      return new Response('ok', { status: 200 })
    })

    const delivery = new WebhookDelivery({ fetch: mockFetch })
    const config: WebhookConfig = {
      url: 'https://example.com/webhook',
      secret,
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }

    const { engine } = createTestServer()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'chunk', level: 'info', data: { n: 1 } })

    const events = await engine.getEvents(task.id)
    const userEvent = events.find(e => e.type === 'chunk')!
    await delivery.send(userEvent, config)

    // Verify HMAC
    const expectedSig = `sha256=${createHmac('sha256', secret).update(receivedBody).digest('hex')}`
    expect(receivedSignature).toBe(expectedSig)
  })

  it('webhook with filter only receives matching events', async () => {
    const calls: string[] = []
    const mockFetch = vi.fn().mockImplementation(async (_url: string, opts: RequestInit) => {
      const parsed = JSON.parse(opts.body as string) as TaskEvent
      calls.push(parsed.type)
      return new Response('ok', { status: 200 })
    })

    const delivery = new WebhookDelivery({ fetch: mockFetch })
    const config: WebhookConfig = {
      url: 'https://example.com/webhook',
      filter: { types: ['llm.*'] },
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }

    const { engine } = createTestServer()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'llm.done', level: 'info', data: null })

    const events = await engine.getEvents(task.id)
    for (const event of events) {
      await delivery.send(event, config)
    }

    // Only llm.* events should have been sent
    expect(calls).toEqual(['llm.delta', 'llm.done'])
  })

  it('retry succeeds after initial failure', async () => {
    let attempt = 0
    const mockFetch = vi.fn().mockImplementation(async () => {
      attempt++
      if (attempt === 1) return new Response('error', { status: 500 })
      return new Response('ok', { status: 200 })
    })

    const delivery = new WebhookDelivery({ fetch: mockFetch })
    const config: WebhookConfig = {
      url: 'https://example.com/webhook',
      retry: { retries: 2, backoff: 'fixed', initialDelayMs: 10, maxDelayMs: 100, timeoutMs: 5000 },
    }

    const { engine } = createTestServer()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'test', level: 'info', data: null })

    const events = await engine.getEvents(task.id)
    const userEvent = events.find(e => e.type === 'test')!
    await delivery.send(userEvent, config)

    expect(mockFetch).toHaveBeenCalledTimes(2)
  })
})
