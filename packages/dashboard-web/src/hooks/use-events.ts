import { useQuery } from '@tanstack/react-query'
import { useState, useEffect, useRef } from 'react'
import { TaskcastClient } from '@taskcast/client'
import type { SSEEnvelope, SubscribeFilter, Level } from '@taskcast/core'
import { useConnectionStore } from '@/stores/connection'
import { apiFetch } from '@/lib/api'

export function useEventHistory(taskId: string | null) {
  return useQuery({
    queryKey: ['events', taskId],
    queryFn: async () => {
      const res = await apiFetch(`/tasks/${taskId}/events/history`)
      if (!res.ok) throw new Error(`Failed to fetch events: ${res.status}`)
      return res.json()
    },
    enabled: !!taskId,
  })
}

export function useEventStream(taskId: string | null, filter?: { types?: string; levels?: string }) {
  const { baseUrl, jwt } = useConnectionStore()
  const [events, setEvents] = useState<SSEEnvelope[]>([])
  const [isDone, setIsDone] = useState(false)
  const [doneReason, setDoneReason] = useState<string | null>(null)
  const [error, setError] = useState<Error | null>(null)
  const abortRef = useRef<AbortController | null>(null)

  useEffect(() => {
    if (!taskId || !baseUrl) return

    setEvents([])
    setIsDone(false)
    setDoneReason(null)
    setError(null)

    const abort = new AbortController()
    abortRef.current = abort

    const client = new TaskcastClient({
      baseUrl,
      token: jwt ?? undefined,
      fetch: (input, init) => globalThis.fetch(input, { ...init, signal: abort.signal }),
    })

    const subscribeFilter = buildSubscribeFilter(filter)

    let cancelled = false

    client
      .subscribe(taskId, {
        filter: subscribeFilter,
        onEvent: (envelope) => {
          if (!cancelled) setEvents((prev) => [...prev, envelope])
        },
        onDone: (reason) => {
          if (!cancelled) {
            setDoneReason(reason)
            setIsDone(true)
          }
        },
        onError: (err) => {
          if (!cancelled) setError(err)
        },
      })
      .catch((err) => {
        if (!cancelled && !abort.signal.aborted) {
          setError(err instanceof Error ? err : new Error(String(err)))
        }
      })

    return () => {
      cancelled = true
      abort.abort()
      abortRef.current = null
    }
  }, [taskId, baseUrl, jwt, filter?.types, filter?.levels])

  return { events, isDone, doneReason, error }
}

function buildSubscribeFilter(filter?: { types?: string; levels?: string }): SubscribeFilter | undefined {
  if (!filter) return undefined
  const result: SubscribeFilter = {}
  if (filter.types) result.types = filter.types.split(',').map((t) => t.trim())
  if (filter.levels) result.levels = filter.levels.split(',').map((l) => l.trim()) as Level[]
  return Object.keys(result).length > 0 ? result : undefined
}
