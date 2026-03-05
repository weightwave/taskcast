import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiFetch } from '@/lib/api'

export function useWorkersQuery() {
  return useQuery({
    queryKey: ['workers'],
    queryFn: async () => {
      const res = await apiFetch('/workers')
      if (!res.ok) throw new Error(`Failed to fetch workers: ${res.status}`)
      const body = await res.json()
      return body.workers ?? body  // Handle both { workers: [...] } and [...]
    },
    refetchInterval: 5000,
  })
}

export function useDrainWorker() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async ({ workerId, status }: { workerId: string; status: 'draining' | 'idle' }) => {
      const res = await apiFetch(`/workers/${workerId}/status`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ status }),
      })
      if (!res.ok) throw new Error(`Failed to update worker: ${res.status}`)
      return res.json()
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['workers'] }),
  })
}

export function useDisconnectWorker() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (workerId: string) => {
      const res = await apiFetch(`/workers/${workerId}`, { method: 'DELETE' })
      if (!res.ok) throw new Error(`Failed to disconnect worker: ${res.status}`)
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['workers'] }),
  })
}
