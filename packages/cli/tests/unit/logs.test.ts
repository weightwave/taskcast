import { describe, it, expect, vi } from 'vitest'
import { formatEvent, consumeSSE } from '../../src/commands/logs.js'

describe('formatEvent', () => {
  it('formats a regular event with timestamp, type, level, data', () => {
    const event = {
      type: 'llm.delta',
      level: 'info',
      timestamp: new Date('2026-03-07T14:30:02Z').getTime(),
      data: { delta: 'Hello ' },
    }
    const result = formatEvent(event)
    // Should contain time, type padded, level padded, and JSON data
    expect(result).toMatch(/\[\d{2}:\d{2}:\d{2}\]/)
    expect(result).toContain('llm.delta')
    expect(result).toContain('info')
    expect(result).toContain('{"delta":"Hello "}')
  })

  it('formats a done event with reason', () => {
    const event = {
      type: 'taskcast.done',
      level: 'info',
      timestamp: new Date('2026-03-07T14:30:03Z').getTime(),
      data: { reason: 'completed' },
    }
    const result = formatEvent(event)
    expect(result).toMatch(/\[\d{2}:\d{2}:\d{2}\]/)
    expect(result).toContain('[DONE] completed')
    expect(result).not.toContain('info')
  })

  it('formats a done event with taskcast:done type', () => {
    const event = {
      type: 'taskcast:done',
      level: 'info',
      timestamp: Date.now(),
      data: { reason: 'failed' },
    }
    const result = formatEvent(event)
    expect(result).toContain('[DONE] failed')
  })

  it('formats event with taskId prefix for tail output', () => {
    const event = {
      type: 'agent.step',
      level: 'info',
      timestamp: Date.now(),
      data: { step: 3 },
    }
    const taskId = '01JXX1234567890ABCDEF'
    const result = formatEvent(event, taskId)
    expect(result).toContain('01JXX12..  ')
    expect(result).toContain('agent.step')
    expect(result).toContain('{"step":3}')
  })

  it('shows unknown reason when done event has no reason field', () => {
    const event = {
      type: 'taskcast.done',
      level: 'info',
      timestamp: Date.now(),
      data: {},
    }
    const result = formatEvent(event)
    expect(result).toContain('[DONE] unknown')
  })

  it('shows unknown reason when done event data is not an object', () => {
    const event = {
      type: 'taskcast.done',
      level: 'info',
      timestamp: Date.now(),
      data: 'just a string',
    }
    const result = formatEvent(event)
    expect(result).toContain('[DONE] unknown')
  })

  it('shows unknown reason when done event data is null', () => {
    const event = {
      type: 'taskcast.done',
      level: 'info',
      timestamp: Date.now(),
      data: null,
    }
    const result = formatEvent(event)
    expect(result).toContain('[DONE] unknown')
  })

  it('pads type to 16 characters for alignment', () => {
    const event = {
      type: 'x',
      level: 'warn',
      timestamp: Date.now(),
      data: {},
    }
    const result = formatEvent(event)
    // type 'x' should be padded to 16 chars
    expect(result).toContain('x               ')
  })

  it('pads level to 5 characters for alignment', () => {
    const event = {
      type: 'llm.delta',
      level: 'info',
      timestamp: Date.now(),
      data: {},
    }
    const result = formatEvent(event)
    // level 'info' should be padded to 5 chars
    expect(result).toContain('info ')
  })

  it('handles null data in regular event', () => {
    const event = {
      type: 'test.event',
      level: 'info',
      timestamp: Date.now(),
      data: null,
    }
    const result = formatEvent(event)
    expect(result).toContain('null')
  })

  it('handles taskId prefix with done event', () => {
    const event = {
      type: 'taskcast.done',
      level: 'info',
      timestamp: Date.now(),
      data: { reason: 'timeout' },
    }
    const result = formatEvent(event, '01JYYZZZZ0000000000000')
    expect(result).toContain('01JYYZZ..  ')
    expect(result).toContain('[DONE] timeout')
  })
})

describe('consumeSSE', () => {
  function makeSSEResponse(chunks: string[]): Response {
    let chunkIndex = 0
    const encoder = new TextEncoder()

    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (chunkIndex < chunks.length) {
          controller.enqueue(encoder.encode(chunks[chunkIndex]))
          chunkIndex++
        } else {
          controller.close()
        }
      },
    })

    return {
      ok: true,
      status: 200,
      body: stream,
    } as unknown as Response
  }

  it('parses SSE events and calls onEvent', async () => {
    const events: Array<{ data: Record<string, unknown>; name: string }> = []
    const mockFetch = async () =>
      makeSSEResponse([
        'event: taskcast.event\ndata: {"type":"llm.delta","level":"info","timestamp":1000,"data":{"delta":"hi"}}\n\n',
      ])

    await consumeSSE(
      'http://localhost/tasks/123/events',
      undefined,
      (event, name) => events.push({ data: event, name }),
      undefined,
      mockFetch as typeof fetch,
    )

    expect(events).toHaveLength(1)
    expect(events[0].name).toBe('taskcast.event')
    expect(events[0].data).toEqual({
      type: 'llm.delta',
      level: 'info',
      timestamp: 1000,
      data: { delta: 'hi' },
    })
  })

  it('calls onDone when taskcast.done event is received', async () => {
    const doneCalled = vi.fn()
    const mockFetch = async () =>
      makeSSEResponse([
        'event: taskcast.done\ndata: {"reason":"completed"}\n\n',
      ])

    await consumeSSE(
      'http://localhost/tasks/123/events',
      undefined,
      () => {},
      doneCalled,
      mockFetch as typeof fetch,
    )

    expect(doneCalled).toHaveBeenCalledOnce()
  })

  it('throws on non-OK response', async () => {
    const mockFetch = async () => ({ ok: false, status: 404 }) as Response

    await expect(
      consumeSSE('http://localhost/tasks/123/events', undefined, () => {}, undefined, mockFetch as typeof fetch),
    ).rejects.toThrow('HTTP 404')
  })

  it('throws on missing response body', async () => {
    const mockFetch = async () => ({ ok: true, status: 200, body: null }) as unknown as Response

    await expect(
      consumeSSE('http://localhost/tasks/123/events', undefined, () => {}, undefined, mockFetch as typeof fetch),
    ).rejects.toThrow('No response body')
  })

  it('sends Authorization header when token is provided', async () => {
    let capturedHeaders: Record<string, string> | undefined
    const mockFetch = async (_url: string, init?: RequestInit) => {
      capturedHeaders = init?.headers as Record<string, string>
      return makeSSEResponse([])
    }

    await consumeSSE(
      'http://localhost/events',
      'my-jwt-token',
      () => {},
      undefined,
      mockFetch as unknown as typeof fetch,
    )

    expect(capturedHeaders?.['Authorization']).toBe('Bearer my-jwt-token')
  })

  it('does not send Authorization header when token is undefined', async () => {
    let capturedHeaders: Record<string, string> | undefined
    const mockFetch = async (_url: string, init?: RequestInit) => {
      capturedHeaders = init?.headers as Record<string, string>
      return makeSSEResponse([])
    }

    await consumeSSE(
      'http://localhost/events',
      undefined,
      () => {},
      undefined,
      mockFetch as unknown as typeof fetch,
    )

    expect(capturedHeaders?.['Authorization']).toBeUndefined()
  })

  it('handles SSE data split across multiple chunks', async () => {
    const events: Array<Record<string, unknown>> = []
    const mockFetch = async () =>
      makeSSEResponse([
        'event: taskcast.event\n',
        'data: {"type":"llm.delta","level":"info","timestamp":1000,"data":{"delta":"hello"}}\n',
        '\n',
      ])

    await consumeSSE(
      'http://localhost/tasks/123/events',
      undefined,
      (event) => events.push(event),
      undefined,
      mockFetch as typeof fetch,
    )

    expect(events).toHaveLength(1)
    expect((events[0] as Record<string, unknown>).type).toBe('llm.delta')
  })

  it('handles multiple events in a single chunk', async () => {
    const events: Array<Record<string, unknown>> = []
    const mockFetch = async () =>
      makeSSEResponse([
        'event: taskcast.event\ndata: {"type":"a","level":"info","timestamp":1,"data":null}\n\nevent: taskcast.event\ndata: {"type":"b","level":"warn","timestamp":2,"data":null}\n\n',
      ])

    await consumeSSE(
      'http://localhost/tasks/123/events',
      undefined,
      (event) => events.push(event),
      undefined,
      mockFetch as typeof fetch,
    )

    expect(events).toHaveLength(2)
    expect((events[0] as Record<string, unknown>).type).toBe('a')
    expect((events[1] as Record<string, unknown>).type).toBe('b')
  })

  it('skips SSE comment lines (starting with colon)', async () => {
    const events: Array<Record<string, unknown>> = []
    const mockFetch = async () =>
      makeSSEResponse([
        ':keepalive\n\nevent: taskcast.event\ndata: {"type":"x","level":"info","timestamp":1,"data":null}\n\n',
      ])

    await consumeSSE(
      'http://localhost/tasks/123/events',
      undefined,
      (event) => events.push(event),
      undefined,
      mockFetch as typeof fetch,
    )

    // The keepalive comment has empty data, so it should not produce an event
    expect(events).toHaveLength(1)
    expect((events[0] as Record<string, unknown>).type).toBe('x')
  })

  it('skips unparseable JSON data gracefully', async () => {
    const events: Array<Record<string, unknown>> = []
    const mockFetch = async () =>
      makeSSEResponse([
        'event: taskcast.event\ndata: {INVALID JSON}\n\nevent: taskcast.event\ndata: {"type":"ok","level":"info","timestamp":1,"data":null}\n\n',
      ])

    await consumeSSE(
      'http://localhost/tasks/123/events',
      undefined,
      (event) => events.push(event),
      undefined,
      mockFetch as typeof fetch,
    )

    expect(events).toHaveLength(1)
    expect((events[0] as Record<string, unknown>).type).toBe('ok')
  })
})
