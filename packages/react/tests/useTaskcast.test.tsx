import { describe, it, expect, vi } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { useTaskEvents } from '../src/index.js'
import type { SSEEnvelope } from '@taskcast/core'

// Mock TaskcastClient
vi.mock('@taskcast/client', () => ({
  TaskcastClient: vi.fn().mockImplementation(() => ({
    subscribe: vi.fn((_taskId: string, opts: { onEvent: (e: SSEEnvelope) => void; onDone: (r: string) => void }) => {
      return new Promise<void>((resolve) => {
        setTimeout(() => {
          opts.onEvent({
            filteredIndex: 0,
            rawIndex: 0,
            eventId: 'e1',
            taskId: 'task-1',
            type: 'llm.delta',
            timestamp: 1000,
            level: 'info',
            data: { text: 'hello' },
          })
          opts.onDone('completed')
          resolve()
        }, 0)
      })
    }),
  })),
}))

describe('useTaskEvents', () => {
  it('subscribes to task and collects events', async () => {
    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )

    await waitFor(() => expect(result.current.isDone).toBe(true))

    expect(result.current.events).toHaveLength(1)
    expect(result.current.events[0]?.type).toBe('llm.delta')
    expect(result.current.doneReason).toBe('completed')
    expect(result.current.error).toBeNull()
  })

  it('initializes with empty state', () => {
    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )
    expect(result.current.events).toEqual([])
    expect(result.current.isDone).toBe(false)
    expect(result.current.error).toBeNull()
  })

  it('does not subscribe when enabled=false', async () => {
    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast', enabled: false })
    )

    // Give a moment for any potential subscriptions
    await new Promise(resolve => setTimeout(resolve, 50))

    // With enabled=false, the effect returns early before creating a client
    expect(result.current.events).toEqual([])
    expect(result.current.isDone).toBe(false)
  })

  it('captures errors from subscribe promise rejection', async () => {
    const { TaskcastClient } = await import('@taskcast/client')
    vi.mocked(TaskcastClient).mockImplementationOnce(() => ({
      subscribe: vi.fn().mockRejectedValue(new Error('Network error')),
    }))

    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )

    await waitFor(() => expect(result.current.error).not.toBeNull())
    expect(result.current.error?.message).toBe('Network error')
  })

  it('captures non-Error rejections from subscribe as Error', async () => {
    const { TaskcastClient } = await import('@taskcast/client')
    vi.mocked(TaskcastClient).mockImplementationOnce(() => ({
      subscribe: vi.fn().mockRejectedValue('string error'),
    }))

    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )

    await waitFor(() => expect(result.current.error).not.toBeNull())
    expect(result.current.error).toBeInstanceOf(Error)
    expect(result.current.error?.message).toBe('string error')
  })

  it('captures errors from onError callback', async () => {
    const { TaskcastClient } = await import('@taskcast/client')
    vi.mocked(TaskcastClient).mockImplementationOnce(() => ({
      subscribe: vi.fn((_taskId: string, opts: { onError?: (e: Error) => void }) => {
        return new Promise<void>((resolve) => {
          setTimeout(() => {
            if (opts.onError) opts.onError(new Error('onError callback'))
            resolve()
          }, 0)
        })
      }),
    }))

    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )

    await waitFor(() => expect(result.current.error).not.toBeNull())
    expect(result.current.error?.message).toBe('onError callback')
  })

  it('passes filter option when provided', async () => {
    const { TaskcastClient } = await import('@taskcast/client')
    const subscribeMock = vi.fn((_taskId: string, _opts: unknown) => {
      return new Promise<void>((resolve) => setTimeout(resolve, 0))
    })
    vi.mocked(TaskcastClient).mockImplementationOnce(() => ({
      subscribe: subscribeMock,
    }))

    renderHook(() =>
      useTaskEvents('task-1', {
        baseUrl: 'http://taskcast',
        filter: { types: ['llm.delta'] },
      })
    )

    await new Promise(resolve => setTimeout(resolve, 20))
    expect(subscribeMock).toHaveBeenCalledWith(
      'task-1',
      expect.objectContaining({ filter: { types: ['llm.delta'] } }),
    )
  })
})
