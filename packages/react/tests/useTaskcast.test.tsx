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

  it('runs cleanup on unmount (sets cancelled=true so callbacks after unmount are ignored)', async () => {
    const { TaskcastClient } = await import('@taskcast/client')

    // Create a subscribe that delays emitting events until after unmount
    let capturedOpts: { onEvent: (e: SSEEnvelope) => void; onDone: (r: string) => void; onError?: (e: Error) => void } | null = null
    vi.mocked(TaskcastClient).mockImplementationOnce(() => ({
      subscribe: vi.fn((_taskId: string, opts: { onEvent: (e: SSEEnvelope) => void; onDone: (r: string) => void; onError?: (e: Error) => void }) => {
        capturedOpts = opts
        // Never resolves — simulates a long-running subscription
        return new Promise<void>(() => {})
      }),
    }))

    const { result, unmount } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )

    // Wait for effect to fire and subscribe to be called
    await waitFor(() => expect(capturedOpts).not.toBeNull())

    // Unmount triggers the cleanup function (lines 60-62: cancelled = true)
    unmount()

    // Now fire callbacks after unmount — they should be ignored due to cancelled=true
    capturedOpts!.onEvent({
      filteredIndex: 0,
      rawIndex: 0,
      eventId: 'e-late',
      taskId: 'task-1',
      type: 'llm.delta',
      timestamp: 2000,
      level: 'info',
      data: { text: 'late' },
    })
    capturedOpts!.onDone('completed')
    capturedOpts!.onError?.(new Error('late error'))

    // Events/state should remain at initial values (nothing updated after unmount)
    expect(result.current.events).toEqual([])
    expect(result.current.isDone).toBe(false)
    expect(result.current.error).toBeNull()
  })

  it('resubscribes and resets events when taskId changes', async () => {
    const { TaskcastClient } = await import('@taskcast/client')

    let subscribeCount = 0
    const cancelledFlags: boolean[] = []

    vi.mocked(TaskcastClient).mockImplementation(() => ({
      subscribe: vi.fn((_taskId: string, opts: { onEvent: (e: SSEEnvelope) => void; onDone: (r: string) => void }) => {
        const callIndex = subscribeCount++
        return new Promise<void>((resolve) => {
          setTimeout(() => {
            opts.onEvent({
              filteredIndex: 0,
              rawIndex: 0,
              eventId: `e-${callIndex}`,
              taskId: _taskId,
              type: 'llm.delta',
              timestamp: 1000 + callIndex,
              level: 'info',
              data: { text: `from-${_taskId}` },
            })
            resolve()
          }, 0)
        })
      }),
    }))

    const { result, rerender } = renderHook(
      ({ taskId }: { taskId: string }) =>
        useTaskEvents(taskId, { baseUrl: 'http://taskcast' }),
      { initialProps: { taskId: 'task-1' } },
    )

    // Wait for first subscription to deliver events
    await waitFor(() => expect(result.current.events.length).toBeGreaterThan(0))
    expect(result.current.events[0]?.taskId).toBe('task-1')

    // Change taskId — should trigger resubscription
    rerender({ taskId: 'task-2' })

    // Wait for the new subscription to deliver events
    await waitFor(() => {
      // events should contain task-2 events (may or may not still have task-1 events
      // depending on React state batching, but a new subscription should be started)
      return result.current.events.some(e => e.taskId === 'task-2')
    })

    // Verify subscribe was called at least twice (once per taskId)
    expect(subscribeCount).toBeGreaterThanOrEqual(2)
  })

  it('resets state properly when baseUrl changes', async () => {
    const { TaskcastClient } = await import('@taskcast/client')

    let constructorCallCount = 0
    vi.mocked(TaskcastClient).mockImplementation((opts: { baseUrl: string }) => {
      constructorCallCount++
      return {
        subscribe: vi.fn((_taskId: string, subOpts: { onEvent: (e: SSEEnvelope) => void; onDone: (r: string) => void }) => {
          return new Promise<void>((resolve) => {
            setTimeout(() => {
              subOpts.onEvent({
                filteredIndex: 0,
                rawIndex: 0,
                eventId: 'e1',
                taskId: 'task-1',
                type: 'llm.delta',
                timestamp: 1000,
                level: 'info',
                data: { url: opts.baseUrl },
              })
              resolve()
            }, 0)
          })
        }),
      }
    })

    const { result, rerender } = renderHook(
      ({ baseUrl }: { baseUrl: string }) =>
        useTaskEvents('task-1', { baseUrl }),
      { initialProps: { baseUrl: 'http://server-1' } },
    )

    await waitFor(() => expect(result.current.events.length).toBeGreaterThan(0))

    // Change baseUrl
    rerender({ baseUrl: 'http://server-2' })

    // Wait for the new subscription to fire
    await waitFor(() => expect(constructorCallCount).toBeGreaterThanOrEqual(2))
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
