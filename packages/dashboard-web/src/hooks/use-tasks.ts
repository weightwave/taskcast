import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiFetch } from '@/lib/api'

interface TaskFilter {
  status?: string
  type?: string
}

export function useTasksQuery(filter?: TaskFilter) {
  return useQuery({
    queryKey: ['tasks', filter],
    queryFn: async () => {
      const params = new URLSearchParams()
      if (filter?.status) params.set('status', filter.status)
      if (filter?.type) params.set('type', filter.type)
      const qs = params.toString()
      const res = await apiFetch(`/tasks${qs ? `?${qs}` : ''}`)
      if (!res.ok) throw new Error(`Failed to fetch tasks: ${res.status}`)
      const body = await res.json()
      return body.tasks as unknown[]
    },
    refetchInterval: 3000,
  })
}

export function useTaskQuery(taskId: string | null) {
  return useQuery({
    queryKey: ['task', taskId],
    queryFn: async () => {
      const res = await apiFetch(`/tasks/${taskId}`)
      if (!res.ok) throw new Error(`Failed to fetch task: ${res.status}`)
      return res.json()
    },
    enabled: !!taskId,
    refetchInterval: 3000,
  })
}

export function useCreateTask() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (input: { type?: string; params?: Record<string, unknown>; ttl?: number; tags?: string[] }) => {
      const res = await apiFetch('/tasks', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(input),
      })
      if (!res.ok) throw new Error(`Failed to create task: ${res.status}`)
      return res.json()
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['tasks'] }),
  })
}

export function useTransitionTask() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async ({ taskId, status, result, error }: { taskId: string; status: string; result?: unknown; error?: unknown }) => {
      const res = await apiFetch(`/tasks/${taskId}/status`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ status, result, error }),
      })
      if (!res.ok) throw new Error(`Failed to transition task: ${res.status}`)
      return res.json()
    },
    onSuccess: (_, { taskId }) => {
      queryClient.invalidateQueries({ queryKey: ['tasks'] })
      queryClient.invalidateQueries({ queryKey: ['task', taskId] })
    },
  })
}
