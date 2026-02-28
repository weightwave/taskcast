import type { TaskcastHooks, Task, TaskError, TaskEvent, ErrorContext } from '@taskcast/core'

interface SentryLike {
  captureException(err: unknown, opts?: {
    tags?: Record<string, string>
    extra?: Record<string, unknown>
  }): void
}

export interface SentryHooksOptions {
  captureTaskFailures?: boolean
  captureTaskTimeouts?: boolean
  captureUnhandledErrors?: boolean
  captureDroppedEvents?: boolean
  captureStorageErrors?: boolean
  captureBroadcastErrors?: boolean
  traceSSEConnections?: boolean
  traceEventPublish?: boolean
}

const DEFAULT_OPTIONS: Required<SentryHooksOptions> = {
  captureTaskFailures: true,
  captureTaskTimeouts: true,
  captureUnhandledErrors: true,
  captureDroppedEvents: true,
  captureStorageErrors: true,
  captureBroadcastErrors: true,
  traceSSEConnections: false,
  traceEventPublish: false,
}

export function createSentryHooks(
  sentry: SentryLike,
  opts: SentryHooksOptions = {},
): TaskcastHooks {
  const options = { ...DEFAULT_OPTIONS, ...opts }

  return {
    onTaskFailed(task: Task, error: TaskError) {
      if (!options.captureTaskFailures) return
      const err = new Error(`Task failed [${task.id}]: ${error.message}`)
      sentry.captureException(err, {
        tags: { taskId: task.id, status: task.status, errorCode: error.code ?? 'unknown' },
        extra: { params: task.params, error: task.error },
      })
    },

    onTaskTimeout(task: Task) {
      if (!options.captureTaskTimeouts) return
      const err = new Error(`Task timed out [${task.id}]`)
      sentry.captureException(err, {
        tags: { taskId: task.id, status: 'timeout' },
        extra: { params: task.params },
      })
    },

    onUnhandledError(err: unknown, context: ErrorContext) {
      if (!options.captureUnhandledErrors) return
      sentry.captureException(err, {
        tags: { operation: context.operation, ...(context.taskId ? { taskId: context.taskId } : {}) },
      })
    },

    onEventDropped(event: TaskEvent, reason: string) {
      if (!options.captureDroppedEvents) return
      const err = new Error(`Event dropped [${event.id}]: ${reason}`)
      sentry.captureException(err, {
        tags: { taskId: event.taskId, eventType: event.type },
        extra: { reason, eventId: event.id },
      })
    },

    onWebhookFailed(config, err) {
      if (!options.captureDroppedEvents) return
      sentry.captureException(err, {
        tags: { webhookUrl: config.url },
      })
    },
  }
}
