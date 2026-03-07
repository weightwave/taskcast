import { describe, it, expect, vi } from 'vitest'
import { TaskcastClient } from '../src/client.js'
import type { SSEEnvelope } from '@taskcast/core'

function makeSSEResponse(lines: string[]): Response {
  const body = lines.join('\n') + '\n'
  return new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'text/event-stream' },
  })
}

function sseEvent(type: string, data: unknown, id?: string): string[] {
  const lines = [`event: ${type}`]
  if (id) lines.push(`id: ${id}`)
  lines.push(`data: ${JSON.stringify(data)}`)
  lines.push('')
  return lines
}

const mockEnvelope: SSEEnvelope = {
  filteredIndex: 0,
  rawIndex: 0,
  eventId: 'evt-1',
  taskId: 'task-abc',
  type: 'log',
  timestamp: 1000,
  level: 'info',
  data: { message: 'hello' },
}

describe('TaskcastClient', () => {
  describe('subscribe', () => {
    it('parses taskcast.event and calls onEvent with the parsed envelope', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const lines = sseEvent('taskcast.event', mockEnvelope)
      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse(lines))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-abc', { onEvent, onDone })

      expect(onEvent).toHaveBeenCalledTimes(1)
      expect(onEvent).toHaveBeenCalledWith(mockEnvelope)
      expect(onDone).not.toHaveBeenCalled()
    })

    it('calls onDone with reason when taskcast.done received', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const lines = sseEvent('taskcast.done', { reason: 'completed' })
      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse(lines))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-abc', { onEvent, onDone })

      expect(onDone).toHaveBeenCalledTimes(1)
      expect(onDone).toHaveBeenCalledWith('completed')
      expect(onEvent).not.toHaveBeenCalled()
    })

    it('passes filter query params: types, levels, since.index', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse([]))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-xyz', {
        onEvent,
        onDone,
        filter: {
          types: ['log', 'progress'],
          levels: ['info', 'warn'],
          since: { index: 5 },
        },
      })

      const calledUrl = mockFetch.mock.calls[0]?.[0] as string
      expect(calledUrl).toContain('types=log%2Cprogress')
      expect(calledUrl).toContain('levels=info%2Cwarn')
      expect(calledUrl).toContain('since.index=5')
    })

    it('throws when response is not OK (e.g., 404)', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const mockFetch = vi.fn().mockResolvedValue(
        new Response('Not Found', { status: 404 })
      )

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })

      await expect(
        client.subscribe('task-missing', { onEvent, onDone })
      ).rejects.toThrow('Failed to subscribe: HTTP 404')
    })

    it('includes Authorization header when token is set', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse([]))

      const client = new TaskcastClient({
        baseUrl: 'http://localhost:3000',
        token: 'my-secret-token',
        fetch: mockFetch,
      })
      await client.subscribe('task-abc', { onEvent, onDone })

      const calledHeaders = mockFetch.mock.calls[0]?.[1]?.headers as Record<string, string>
      expect(calledHeaders['Authorization']).toBe('Bearer my-secret-token')
    })

    it('ignores unknown SSE event types (no onEvent/onDone call)', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const lines = sseEvent('some.unknown.event', { foo: 'bar' })
      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse(lines))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-abc', { onEvent, onDone })

      expect(onEvent).not.toHaveBeenCalled()
      expect(onDone).not.toHaveBeenCalled()
    })

    it('silently ignores invalid JSON in taskcast.event data', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      // Send raw SSE with invalid JSON for a taskcast.event
      const lines = [
        'event: taskcast.event',
        'data: not-valid-json{{{',
        '',
      ]
      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse(lines))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-abc', { onEvent, onDone })

      // Should not crash and should not call onEvent
      expect(onEvent).not.toHaveBeenCalled()
      expect(onDone).not.toHaveBeenCalled()
    })

    it('silently ignores invalid JSON in taskcast.done data', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      // Send raw SSE with invalid JSON for a taskcast.done
      const lines = [
        'event: taskcast.done',
        'data: <<<broken>>>',
        '',
      ]
      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse(lines))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-abc', { onEvent, onDone })

      // Should not crash and should not call onDone
      expect(onEvent).not.toHaveBeenCalled()
      expect(onDone).not.toHaveBeenCalled()
    })

    it('handles multiple SSE events in a stream', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const envelope2: SSEEnvelope = { ...mockEnvelope, filteredIndex: 1, rawIndex: 1, eventId: 'evt-2' }
      const lines = [
        ...sseEvent('taskcast.event', mockEnvelope),
        ...sseEvent('taskcast.event', envelope2),
        ...sseEvent('taskcast.done', { reason: 'finished' }),
      ]
      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse(lines))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-abc', { onEvent, onDone })

      expect(onEvent).toHaveBeenCalledTimes(2)
      expect(onDone).toHaveBeenCalledWith('finished')
    })

    it('does not include Authorization header when no token is set', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const mockFetch = vi.fn().mockResolvedValue(makeSSEResponse([]))

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })
      await client.subscribe('task-abc', { onEvent, onDone })

      const calledHeaders = mockFetch.mock.calls[0]?.[1]?.headers as Record<string, string>
      expect(calledHeaders['Authorization']).toBeUndefined()
    })
  })

  describe('non-200 HTTP responses', () => {
    it('throws error with status info on HTTP 500', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const mockFetch = vi.fn().mockResolvedValue(
        new Response('Internal Server Error', { status: 500 })
      )

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })

      await expect(
        client.subscribe('task-abc', { onEvent, onDone })
      ).rejects.toThrow('Failed to subscribe: HTTP 500')
    })

    it('throws error with status info on HTTP 404', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const mockFetch = vi.fn().mockResolvedValue(
        new Response('Not Found', { status: 404 })
      )

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })

      const err = await client.subscribe('task-abc', { onEvent, onDone }).catch(e => e)
      expect(err).toBeInstanceOf(Error)
      expect(err.message).toContain('404')
    })

    it('throws "No response body" when status 200 but body is null', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        body: null,
      })

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })

      await expect(
        client.subscribe('task-abc', { onEvent, onDone })
      ).rejects.toThrow('No response body')
    })
  })

  describe('AbortController mid-stream', () => {
    it('resolves cleanly when reader is aborted mid-stream', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      // Create a readable stream that we can control
      let controller!: ReadableStreamDefaultController<Uint8Array>
      const stream = new ReadableStream<Uint8Array>({
        start(ctrl) {
          controller = ctrl
        },
      })

      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        body: stream,
      })

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })

      // Start subscribe in background
      const subscribePromise = client.subscribe('task-abc', { onEvent, onDone })

      // Feed some data, then close the stream
      const encoder = new TextEncoder()
      controller.enqueue(encoder.encode('event: taskcast.event\ndata: {"filteredIndex":0,"rawIndex":0,"eventId":"e1","taskId":"t1","type":"log","timestamp":1000,"level":"info","data":{}}\n\n'))
      controller.close()

      // subscribe should resolve cleanly
      await subscribePromise

      expect(onEvent).toHaveBeenCalledTimes(1)
    })

    it('rejects when stream errors mid-read', async () => {
      const onEvent = vi.fn()
      const onDone = vi.fn()

      // Create a readable stream that errors immediately
      let controller!: ReadableStreamDefaultController<Uint8Array>
      const stream = new ReadableStream<Uint8Array>({
        start(ctrl) {
          controller = ctrl
        },
      })

      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        body: stream,
      })

      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: mockFetch })

      const subscribePromise = client.subscribe('task-abc', { onEvent, onDone })

      // Error the stream — per spec, controller.error() clears the queue
      controller.error(new Error('Connection reset'))

      // subscribe should reject with the stream error
      await expect(subscribePromise).rejects.toThrow('Connection reset')
    })
  })

  describe('_buildURL', () => {
    it('returns plain URL with no query string when no filter given', () => {
      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: vi.fn() })
      // Access private method via type cast
      const url = (client as unknown as { _buildURL(taskId: string, filter?: unknown): string })._buildURL('task-123')
      expect(url).toBe('http://localhost:3000/tasks/task-123/events')
      expect(url).not.toContain('?')
    })

    it('strips trailing slash from baseUrl', () => {
      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000/', fetch: vi.fn() })
      const url = (client as unknown as { _buildURL(taskId: string, filter?: unknown): string })._buildURL('task-123')
      expect(url).toBe('http://localhost:3000/tasks/task-123/events')
    })

    it('sets includeStatus=false when filter.includeStatus is false', () => {
      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: vi.fn() })
      const url = (client as unknown as { _buildURL(taskId: string, filter?: unknown): string })._buildURL('task-123', { includeStatus: false })
      expect(url).toContain('includeStatus=false')
    })

    it('sets wrap=false when filter.wrap is false', () => {
      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: vi.fn() })
      const url = (client as unknown as { _buildURL(taskId: string, filter?: unknown): string })._buildURL('task-123', { wrap: false })
      expect(url).toContain('wrap=false')
    })

    it('sets since.id when filter.since.id is provided', () => {
      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: vi.fn() })
      const url = (client as unknown as { _buildURL(taskId: string, filter?: unknown): string })._buildURL('task-123', { since: { id: 'evt-abc' } })
      expect(url).toContain('since.id=evt-abc')
    })

    it('sets since.timestamp when filter.since.timestamp is provided', () => {
      const client = new TaskcastClient({ baseUrl: 'http://localhost:3000', fetch: vi.fn() })
      const url = (client as unknown as { _buildURL(taskId: string, filter?: unknown): string })._buildURL('task-123', { since: { timestamp: 12345 } })
      expect(url).toContain('since.timestamp=12345')
    })
  })
})
