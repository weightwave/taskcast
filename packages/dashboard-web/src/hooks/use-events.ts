import { useQuery } from '@tanstack/react-query'
import { useState, useEffect, useRef } from 'react'
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
  const [events, setEvents] = useState<unknown[]>([])
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

    const params = new URLSearchParams()
    if (filter?.types) params.set('types', filter.types)
    if (filter?.levels) params.set('levels', filter.levels)
    const qs = params.toString()

    const url = `${baseUrl}/tasks/${taskId}/events${qs ? `?${qs}` : ''}`

    async function connect() {
      try {
        const headers: Record<string, string> = { Accept: 'text/event-stream' }
        if (jwt) headers['Authorization'] = `Bearer ${jwt}`

        const res = await fetch(url, { headers, signal: abort.signal })
        if (!res.ok || !res.body) {
          throw new Error(`SSE connection failed: ${res.status}`)
        }

        const reader = res.body.getReader()
        const decoder = new TextDecoder()
        let buffer = ''

        while (true) {
          const { done, value } = await reader.read()
          if (done) break

          buffer += decoder.decode(value, { stream: true })
          const lines = buffer.split('\n')
          buffer = lines.pop() ?? ''

          let currentEvent = ''
          let currentData = ''

          for (const line of lines) {
            if (line.startsWith('event: ')) {
              currentEvent = line.slice(7).trim()
            } else if (line.startsWith('data: ')) {
              currentData = line.slice(6)
            } else if (line === '') {
              if (currentEvent === 'taskcast.done') {
                try {
                  const parsed = JSON.parse(currentData)
                  setDoneReason(parsed.reason ?? 'done')
                } catch {
                  setDoneReason('done')
                }
                setIsDone(true)
              } else if (currentEvent === 'taskcast.event' && currentData) {
                try {
                  const parsed = JSON.parse(currentData)
                  setEvents((prev) => [...prev, parsed])
                } catch {
                  // skip unparseable
                }
              }
              currentEvent = ''
              currentData = ''
            }
          }
        }
      } catch (err) {
        if (!abort.signal.aborted) {
          setError(err instanceof Error ? err : new Error(String(err)))
        }
      }
    }

    connect()

    return () => {
      abort.abort()
      abortRef.current = null
    }
  }, [taskId, baseUrl, jwt, filter?.types, filter?.levels])

  return { events, isDone, doneReason, error }
}
