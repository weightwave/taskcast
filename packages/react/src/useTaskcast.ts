import { useState, useEffect } from 'react'
import { TaskcastClient } from '@taskcast/client'
import type { TaskcastClientOptions, SubscribeOptions } from '@taskcast/client'
import type { SSEEnvelope, SubscribeFilter } from '@taskcast/core'

export interface UseTaskEventsOptions extends TaskcastClientOptions {
  filter?: SubscribeFilter
  enabled?: boolean
}

export interface UseTaskEventsResult {
  events: SSEEnvelope[]
  isDone: boolean
  doneReason: string | null
  error: Error | null
}

export function useTaskEvents(
  taskId: string,
  opts: UseTaskEventsOptions,
): UseTaskEventsResult {
  const [events, setEvents] = useState<SSEEnvelope[]>([])
  const [isDone, setIsDone] = useState(false)
  const [doneReason, setDoneReason] = useState<string | null>(null)
  const [error, setError] = useState<Error | null>(null)

  const enabled = opts.enabled ?? true

  useEffect(() => {
    if (!enabled || !taskId) return

    const clientOpts: TaskcastClientOptions = { baseUrl: opts.baseUrl }
    if (opts.token !== undefined) clientOpts.token = opts.token
    if (opts.fetch !== undefined) clientOpts.fetch = opts.fetch
    const client = new TaskcastClient(clientOpts)

    let cancelled = false

    const subscribeOpts: SubscribeOptions = {
      onEvent: (envelope) => {
        if (!cancelled) setEvents((prev) => [...prev, envelope])
      },
      onDone: (reason) => {
        if (!cancelled) {
          setDoneReason(reason)
          setIsDone(true)
        }
      },
    }
    if (opts.filter !== undefined) subscribeOpts.filter = opts.filter

    subscribeOpts.onError = (err) => {
      if (!cancelled) setError(err)
    }

    client.subscribe(taskId, subscribeOpts).catch((err) => {
      if (!cancelled) setError(err instanceof Error ? err : new Error(String(err)))
    })

    return () => {
      cancelled = true
    }
  }, [taskId, opts.baseUrl, opts.token, enabled])

  return { events, isDone, doneReason, error }
}
