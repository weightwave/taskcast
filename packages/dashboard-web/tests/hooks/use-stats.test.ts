import { describe, it, expect, beforeEach, vi } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import type { ReactNode } from 'react'
import { useConnectionStore } from '@/stores/connection'
import { useStats } from '@/hooks/use-stats'

const mockTasks = {
  tasks: [
    { id: '1', status: 'running', createdAt: 100 },
    { id: '2', status: 'completed', createdAt: 300 },
    { id: '3', status: 'running', createdAt: 200 },
  ],
}

const mockWorkers = {
  workers: [
    { id: 'w1', status: 'idle', capacity: 10, usedSlots: 3 },
    { id: 'w2', status: 'offline', capacity: 5, usedSlots: 0 },
  ],
}

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        gcTime: 0,
      },
    },
  })
  return function Wrapper({ children }: { children: ReactNode }) {
    return createElement(QueryClientProvider, { client: queryClient }, children)
  }
}

describe('useStats', () => {
  beforeEach(() => {
    vi.restoreAllMocks()

    // Set connection store to connected state so apiFetch works
    useConnectionStore.setState({
      baseUrl: 'http://test',
      connected: true,
      jwt: null,
      error: null,
    })

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString()
      if (urlStr.includes('/tasks')) {
        return Response.json(mockTasks, { status: 200 })
      }
      if (urlStr.includes('/workers')) {
        return Response.json(mockWorkers, { status: 200 })
      }
      return new Response('Not Found', { status: 404 })
    })
  })

  it('computes status counts', async () => {
    const { result } = renderHook(() => useStats(), { wrapper: createWrapper() })

    await waitFor(() => {
      expect(result.current.isPending).toBe(false)
    })

    expect(result.current.statusCounts).toEqual({ running: 2, completed: 1 })
  })

  it('sorts recent tasks by createdAt descending', async () => {
    const { result } = renderHook(() => useStats(), { wrapper: createWrapper() })

    await waitFor(() => {
      expect(result.current.isPending).toBe(false)
    })

    expect(result.current.recentTasks[0].id).toBe('2')
    expect(result.current.recentTasks[1].id).toBe('3')
    expect(result.current.recentTasks[2].id).toBe('1')
  })

  it('computes worker capacity', async () => {
    const { result } = renderHook(() => useStats(), { wrapper: createWrapper() })

    await waitFor(() => {
      expect(result.current.isPending).toBe(false)
    })

    expect(result.current.totalCapacity).toBe(15)
    expect(result.current.usedCapacity).toBe(3)
    expect(result.current.onlineWorkers).toBe(1)
  })
})
