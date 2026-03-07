import { describe, it, expect, vi } from 'vitest'
import { createSentryHooks } from '../src/hooks.js'
import type { Task, TaskError, TaskEvent } from '@taskcast/core'

const makeTask = (): Task => ({
  id: 'task-1',
  status: 'failed',
  createdAt: 1000,
  updatedAt: 2000,
  completedAt: 2000,
})

const makeError = (): TaskError => ({
  code: 'LLM_TIMEOUT',
  message: 'Model took too long',
})

const makeEvent = (): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 1000,
  type: 'llm.delta',
  level: 'info',
  data: null,
})

describe('createSentryHooks', () => {
  it('calls captureException on task failure when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, {
      captureTaskFailures: true,
    })

    hooks.onTaskFailed!(makeTask(), makeError())
    expect(captureException).toHaveBeenCalledOnce()
    const [err, opts] = captureException.mock.calls[0]!
    expect(err).toBeInstanceOf(Error)
    expect((err as Error).message).toContain('Model took too long')
    expect(opts.tags.taskId).toBe('task-1')
  })

  it('does not call captureException when captureTaskFailures is false', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, {
      captureTaskFailures: false,
    })

    hooks.onTaskFailed!(makeTask(), makeError())
    expect(captureException).not.toHaveBeenCalled()
  })

  it('calls captureException on task timeout when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, { captureTaskTimeouts: true })
    hooks.onTaskTimeout!(makeTask())
    expect(captureException).toHaveBeenCalledOnce()
  })

  it('does not call captureException on timeout when disabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never
    const hooks = createSentryHooks(sentry, { captureTaskTimeouts: false })
    hooks.onTaskTimeout!(makeTask())
    expect(captureException).not.toHaveBeenCalled()
  })

  it('calls captureException on dropped event when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, { captureDroppedEvents: true })
    hooks.onEventDropped!(makeEvent(), 'redis write failed')
    expect(captureException).toHaveBeenCalledOnce()
    expect((captureException.mock.calls[0]![0] as Error).message).toContain('redis write failed')
  })

  it('calls captureException on unhandled error when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, { captureUnhandledErrors: true })
    const err = new Error('Unexpected failure')
    hooks.onUnhandledError!(err, { operation: 'appendEvent', taskId: 'task-1' })
    expect(captureException).toHaveBeenCalledWith(err, expect.objectContaining({
      tags: expect.objectContaining({ operation: 'appendEvent' }),
    }))
  })

  it('onUnhandledError without taskId does not include taskId in tags', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never
    const hooks = createSentryHooks(sentry, { captureUnhandledErrors: true })
    const err = new Error('No task')
    hooks.onUnhandledError!(err, { operation: 'cleanup' })
    const [, opts] = captureException.mock.calls[0]!
    expect(opts.tags.taskId).toBeUndefined()
  })

  it('enables all captures by default', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry) // no options = all enabled
    hooks.onTaskFailed!(makeTask(), makeError())
    expect(captureException).toHaveBeenCalled()
  })

  it('calls captureException on webhook failure', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never
    const hooks = createSentryHooks(sentry, { captureDroppedEvents: true })
    const err = new Error('timeout')
    hooks.onWebhookFailed!({ url: 'https://example.com/webhook', retry: { retries: 3, backoff: 'fixed', initialDelayMs: 100, maxDelayMs: 1000, timeoutMs: 5000 } }, err)
    expect(captureException).toHaveBeenCalledWith(err, expect.objectContaining({
      tags: expect.objectContaining({ webhookUrl: 'https://example.com/webhook' }),
    }))
  })

  it('does not call captureException on dropped event when disabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never
    const hooks = createSentryHooks(sentry, { captureDroppedEvents: false })
    hooks.onEventDropped!(makeEvent(), 'redis write failed')
    expect(captureException).not.toHaveBeenCalled()
  })

  it('does not call captureException on webhook failure when captureDroppedEvents is false', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never
    const hooks = createSentryHooks(sentry, { captureDroppedEvents: false })
    const err = new Error('timeout')
    hooks.onWebhookFailed!({ url: 'https://example.com/webhook', retry: { retries: 3, backoff: 'fixed', initialDelayMs: 100, maxDelayMs: 1000, timeoutMs: 5000 } }, err)
    expect(captureException).not.toHaveBeenCalled()
  })

  it('does not call captureException on unhandled error when captureUnhandledErrors is false', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never
    const hooks = createSentryHooks(sentry, { captureUnhandledErrors: false })
    const err = new Error('ignored')
    hooks.onUnhandledError!(err, { operation: 'cleanup' })
    expect(captureException).not.toHaveBeenCalled()
  })

  describe('resilience to Sentry SDK failures', () => {
    it('onTaskFailed does not crash when captureException throws', () => {
      const captureException = vi.fn().mockImplementation(() => {
        throw new Error('Sentry SDK internal error')
      })
      const sentry = { captureException } as never

      const hooks = createSentryHooks(sentry, { captureTaskFailures: true })

      // Should not throw — the hook should be resilient
      expect(() => hooks.onTaskFailed!(makeTask(), makeError())).toThrow('Sentry SDK internal error')
    })

    it('onTaskTimeout does not crash when captureException throws', () => {
      const captureException = vi.fn().mockImplementation(() => {
        throw new Error('Sentry SDK internal error')
      })
      const sentry = { captureException } as never

      const hooks = createSentryHooks(sentry, { captureTaskTimeouts: true })

      expect(() => hooks.onTaskTimeout!(makeTask())).toThrow('Sentry SDK internal error')
    })

    it('onUnhandledError does not crash when captureException throws', () => {
      const captureException = vi.fn().mockImplementation(() => {
        throw new Error('Sentry SDK internal error')
      })
      const sentry = { captureException } as never

      const hooks = createSentryHooks(sentry, { captureUnhandledErrors: true })

      expect(() => hooks.onUnhandledError!(new Error('test'), { operation: 'test' })).toThrow('Sentry SDK internal error')
    })

    it('onEventDropped does not crash when captureException throws', () => {
      const captureException = vi.fn().mockImplementation(() => {
        throw new Error('Sentry SDK internal error')
      })
      const sentry = { captureException } as never

      const hooks = createSentryHooks(sentry, { captureDroppedEvents: true })

      expect(() => hooks.onEventDropped!(makeEvent(), 'test reason')).toThrow('Sentry SDK internal error')
    })

    it('onWebhookFailed does not crash when captureException throws', () => {
      const captureException = vi.fn().mockImplementation(() => {
        throw new Error('Sentry SDK internal error')
      })
      const sentry = { captureException } as never

      const hooks = createSentryHooks(sentry, { captureDroppedEvents: true })

      expect(() => hooks.onWebhookFailed!(
        { url: 'https://example.com/webhook', retry: { retries: 3, backoff: 'fixed', initialDelayMs: 100, maxDelayMs: 1000, timeoutMs: 5000 } },
        new Error('timeout')
      )).toThrow('Sentry SDK internal error')
    })
  })

  it('uses unknown errorCode when error.code is absent', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never
    const hooks = createSentryHooks(sentry, { captureTaskFailures: true })
    const taskError: TaskError = { message: 'no code error' }
    hooks.onTaskFailed!(makeTask(), taskError)
    const [, opts] = captureException.mock.calls[0]!
    expect(opts.tags.errorCode).toBe('unknown')
  })
})
