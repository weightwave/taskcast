import { describe, it, expect, vi } from 'vitest'
import { TaskcastServerClient } from '../src/client.js'
import { TaskcastServerClient as TaskcastServerClientFromIndex } from '../src/index.js'

function makeFetch(responses: Array<{ status: number; body: unknown }>) {
  let i = 0
  return vi.fn().mockImplementation(() => {
    const r = responses[i++] ?? { status: 200, body: {} }
    return Promise.resolve(
      new Response(JSON.stringify(r.body), {
        status: r.status,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
  })
}

describe('TaskcastServerClient.createTask', () => {
  it('POST /tasks and returns created task', async () => {
    const fetch = makeFetch([{ status: 201, body: { id: 'task-1', status: 'pending' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const task = await client.createTask({ params: { prompt: 'hi' } })
    expect(task.id).toBe('task-1')
    expect(task.status).toBe('pending')

    expect(fetch).toHaveBeenCalledOnce()
    const [url, opts] = fetch.mock.calls[0]!
    expect(url).toBe('http://taskcast/tasks')
    expect(opts.method).toBe('POST')
    expect(JSON.parse(opts.body)).toEqual({ params: { prompt: 'hi' } })
  })
})

describe('TaskcastServerClient.getTask', () => {
  it('GET /tasks/:id and returns task', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'running' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const task = await client.getTask('task-1')
    expect(task.status).toBe('running')
    expect(fetch.mock.calls[0]![0]).toBe('http://taskcast/tasks/task-1')
  })

  it('throws on 404', async () => {
    const fetch = makeFetch([{ status: 404, body: { error: 'Task not found' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('missing')).rejects.toThrow(/not found/i)
  })
})

describe('TaskcastServerClient.transitionTask', () => {
  it('PATCH /tasks/:id/status', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'running' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    await client.transitionTask('task-1', 'running')
    const [url, opts] = fetch.mock.calls[0]!
    expect(url).toBe('http://taskcast/tasks/task-1/status')
    expect(opts.method).toBe('PATCH')
    expect(JSON.parse(opts.body)).toEqual({ status: 'running' })
  })
})

describe('TaskcastServerClient.publishEvent', () => {
  it('POST /tasks/:id/events single event', async () => {
    const fetch = makeFetch([{ status: 201, body: { id: 'evt-1', type: 'llm.delta' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    await client.publishEvent('task-1', { type: 'llm.delta', level: 'info', data: null })
    const [url] = fetch.mock.calls[0]!
    expect(url).toBe('http://taskcast/tasks/task-1/events')
  })

  it('POST /tasks/:id/events batch', async () => {
    const fetch = makeFetch([{ status: 201, body: [{ id: 'e1' }, { id: 'e2' }] }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const events = await client.publishEvents('task-1', [
      { type: 'a', level: 'info', data: null },
      { type: 'b', level: 'info', data: null },
    ])
    expect(events).toHaveLength(2)
  })
})

describe('Authorization header', () => {
  it('sends Bearer token when configured', async () => {
    const fetch = makeFetch([{ status: 201, body: { id: 'task-1', status: 'pending' } }])
    const client = new TaskcastServerClient({
      baseUrl: 'http://taskcast',
      token: 'my-jwt-token',
      fetch,
    })
    await client.createTask({})
    const [, opts] = fetch.mock.calls[0]!
    expect(opts.headers['Authorization']).toBe('Bearer my-jwt-token')
  })
})

describe('TaskcastServerClient.getHistory', () => {
  it('GET /tasks/:id/events/history with no filter', async () => {
    const fetch = makeFetch([{ status: 200, body: [] }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    const events = await client.getHistory('task-1')
    expect(events).toEqual([])
    expect(fetch.mock.calls[0]![0]).toBe('http://taskcast/tasks/task-1/events/history')
  })

  it('appends since query params', async () => {
    const fetch = makeFetch([{ status: 200, body: [] }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await client.getHistory('task-1', { since: { index: 3 } })
    expect(fetch.mock.calls[0]![0]).toContain('since.index=3')
  })

  it('appends since.id and since.timestamp query params', async () => {
    const fetch = makeFetch([{ status: 200, body: [] }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await client.getHistory('task-1', { since: { id: 'evt-5', timestamp: 1000 } })
    const url = fetch.mock.calls[0]![0] as string
    expect(url).toContain('since.id=evt-5')
    expect(url).toContain('since.timestamp=1000')
  })
})

describe('Error handling with non-JSON body', () => {
  it('throws generic HTTP error when response body is not JSON', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response('Internal Server Error', {
        status: 500,
        headers: { 'Content-Type': 'text/plain' },
      }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('task-1')).rejects.toThrow('HTTP 500')
  })

  it('falls back to "HTTP 404" when JSON body has no .error field', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ code: 'not_found' }), {
        status: 404,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('missing')).rejects.toThrow('HTTP 404')
  })
})

describe('TaskcastServerClient.transitionTask with payload', () => {
  it('includes result and error fields in request body', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'done' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const result = { answer: 42 }
    const error = { code: 'timeout', message: 'timed out' }
    await client.transitionTask('task-1', 'done', { result, error })

    const [, opts] = fetch.mock.calls[0]!
    expect(JSON.parse(opts.body)).toEqual({ status: 'done', result, error })
  })
})

describe('Network failures and error responses', () => {
  it('propagates fetch TypeError from createTask', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.createTask({})).rejects.toThrow('fetch failed')
    await expect(client.createTask({})).rejects.toBeInstanceOf(TypeError)
  })

  it('propagates fetch TypeError from getTask', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('task-1')).rejects.toThrow('fetch failed')
  })

  it('propagates fetch TypeError from transitionTask', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.transitionTask('task-1', 'running')).rejects.toThrow('fetch failed')
  })

  it('propagates fetch TypeError from publishEvent', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(
      client.publishEvent('task-1', { type: 'log', level: 'info', data: null })
    ).rejects.toThrow('fetch failed')
  })

  it('propagates fetch TypeError from publishEvents', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(
      client.publishEvents('task-1', [{ type: 'log', level: 'info', data: null }])
    ).rejects.toThrow('fetch failed')
  })

  it('propagates fetch TypeError from getHistory', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getHistory('task-1')).rejects.toThrow('fetch failed')
  })

  it('throws error message from 401 response', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ error: 'Unauthorized' }), {
        status: 401,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('task-1')).rejects.toThrow('Unauthorized')
  })

  it('throws error on 200 with invalid JSON body', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response('not valid json {{{', {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('task-1')).rejects.toThrow()
  })

  it('extracts error message from 500 JSON response', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ error: 'Internal error' }), {
        status: 500,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('task-1')).rejects.toThrow('Internal error')
  })

  it('does not include Authorization header when no token set', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'pending' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await client.getTask('task-1')
    const [, opts] = fetch.mock.calls[0]!
    expect(opts.headers['Authorization']).toBeUndefined()
  })

  it('does not include Content-Type when body is undefined (GET requests)', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'pending' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await client.getTask('task-1')
    const [, opts] = fetch.mock.calls[0]!
    expect(opts.headers['Content-Type']).toBeUndefined()
    expect(opts.body).toBeUndefined()
  })

  it('strips trailing slash from baseUrl', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'pending' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast/', fetch })
    await client.getTask('task-1')
    expect(fetch.mock.calls[0]![0]).toBe('http://taskcast/tasks/task-1')
  })
})

// --- SSE subscribe tests ---

/** Create a ReadableStream that emits SSE-formatted text chunks. */
function makeSSEStream(chunks: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder()
  let i = 0
  return new ReadableStream({
    pull(controller) {
      if (i < chunks.length) {
        controller.enqueue(encoder.encode(chunks[i++]))
      } else {
        controller.close()
      }
    },
  })
}

function makeSSEFetch(sseChunks: string[]) {
  return vi.fn().mockImplementation((url: string) => {
    if (url.includes('/events') && !url.includes('/history')) {
      return Promise.resolve(
        new Response(makeSSEStream(sseChunks), {
          status: 200,
          headers: { 'Content-Type': 'text/event-stream' },
        }),
      )
    }
    return Promise.resolve(new Response(JSON.stringify({}), { status: 200 }))
  })
}

describe('TaskcastServerClient.subscribe', () => {
  it('connects to SSE endpoint and delivers events to handler', async () => {
    const event1 = { id: 'e1', taskId: 't1', index: 0, timestamp: 1000, type: 'log', level: 'info', data: { msg: 'hello' } }
    const event2 = { id: 'e2', taskId: 't1', index: 1, timestamp: 1001, type: 'log', level: 'info', data: { msg: 'world' } }

    const fetch = makeSSEFetch([
      `event: taskcast.event\ndata: ${JSON.stringify(event1)}\n\n`,
      `event: taskcast.event\ndata: ${JSON.stringify(event2)}\n\n`,
    ])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    const received: unknown[] = []

    client.subscribe('t1', (e) => received.push(e))

    // Wait for async SSE consumption
    await new Promise((r) => setTimeout(r, 50))

    expect(received).toHaveLength(2)
    expect(received[0]).toEqual(event1)
    expect(received[1]).toEqual(event2)
  })

  it('builds correct URL with default wrap=false', async () => {
    const fetch = makeSSEFetch([])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    client.subscribe('task-1', () => {})
    await new Promise((r) => setTimeout(r, 10))

    const url = fetch.mock.calls[0]![0] as string
    expect(url).toContain('/tasks/task-1/events')
    expect(url).toContain('wrap=false')
  })

  it('includes filter query params', async () => {
    const fetch = makeSSEFetch([])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    client.subscribe('task-1', () => {}, {
      types: ['llm.*', 'progress'],
      levels: ['info', 'warn'],
      since: { index: 5 },
      seriesFormat: 'accumulated',
    })
    await new Promise((r) => setTimeout(r, 10))

    const url = fetch.mock.calls[0]![0] as string
    expect(url).toContain('types=llm.*%2Cprogress')
    expect(url).toContain('levels=info%2Cwarn')
    expect(url).toContain('since.index=5')
    expect(url).toContain('seriesFormat=accumulated')
  })

  it('sends Authorization header when token is set', async () => {
    const fetch = makeSSEFetch([])
    const client = new TaskcastServerClient({
      baseUrl: 'http://taskcast',
      token: 'my-token',
      fetch,
    })

    client.subscribe('t1', () => {})
    await new Promise((r) => setTimeout(r, 10))

    const headers = fetch.mock.calls[0]![1].headers
    expect(headers['Authorization']).toBe('Bearer my-token')
  })

  it('returns unsubscribe function that aborts the connection', async () => {
    // Stream that never ends
    const stream = new ReadableStream<Uint8Array>({
      start() { /* hang forever */ },
    })
    const fetch = vi.fn().mockResolvedValue(
      new Response(stream, { status: 200, headers: { 'Content-Type': 'text/event-stream' } }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const unsubscribe = client.subscribe('t1', () => {})

    // Should not throw
    unsubscribe()
  })

  it('stops on taskcast.done event', async () => {
    const event1 = { id: 'e1', taskId: 't1', index: 0, timestamp: 1000, type: 'log', level: 'info', data: null }
    const fetch = makeSSEFetch([
      `event: taskcast.event\ndata: ${JSON.stringify(event1)}\n\n`,
      `event: taskcast.done\ndata: {"reason":"completed"}\n\n`,
      // This event should NOT be delivered
      `event: taskcast.event\ndata: ${JSON.stringify({ ...event1, id: 'e2' })}\n\n`,
    ])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    const received: unknown[] = []

    client.subscribe('t1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 50))

    expect(received).toHaveLength(1)
  })

  it('skips malformed JSON data gracefully', async () => {
    const event1 = { id: 'e1', taskId: 't1', index: 0, timestamp: 1000, type: 'log', level: 'info', data: null }
    const fetch = makeSSEFetch([
      `event: taskcast.event\ndata: not-json\n\n`,
      `event: taskcast.event\ndata: ${JSON.stringify(event1)}\n\n`,
    ])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    const received: unknown[] = []

    client.subscribe('t1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 50))

    expect(received).toHaveLength(1)
    expect(received[0]).toEqual(event1)
  })

  it('handles non-200 response gracefully', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ error: 'Not found' }), { status: 404 }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    const received: unknown[] = []

    // Should not throw
    client.subscribe('t1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 50))

    expect(received).toHaveLength(0)
  })

  it('continues streaming when handler throws', async () => {
    const event1 = { id: 'e1', taskId: 't1', index: 0, timestamp: 1000, type: 'log', level: 'info', data: null }
    const event2 = { id: 'e2', taskId: 't1', index: 1, timestamp: 1001, type: 'log', level: 'info', data: null }
    const fetch = makeSSEFetch([
      `event: taskcast.event\ndata: ${JSON.stringify(event1)}\n\n`,
      `event: taskcast.event\ndata: ${JSON.stringify(event2)}\n\n`,
    ])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    const received: unknown[] = []
    let callCount = 0

    client.subscribe('t1', (e) => {
      callCount++
      if (callCount === 1) throw new Error('handler error')
      received.push(e)
    })
    await new Promise((r) => setTimeout(r, 50))

    // Second event should still be delivered despite first handler throwing
    expect(received).toHaveLength(1)
    expect(received[0]).toEqual(event2)
  })

  it('rethrows non-AbortError from reader', async () => {
    let readCount = 0
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        readCount++
        if (readCount === 1) {
          const encoder = new TextEncoder()
          controller.enqueue(encoder.encode('event: taskcast.event\ndata: {"id":"e1"}\n\n'))
        } else {
          throw new Error('unexpected read error')
        }
      },
    })
    const fetch = vi.fn().mockResolvedValue(
      new Response(stream, { status: 200, headers: { 'Content-Type': 'text/event-stream' } }),
    )
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    // _consumeSSE is fire-and-forget, so the rethrown error becomes an unhandled rejection.
    const errors: Error[] = []
    const onReject = (reason: unknown) => { errors.push(reason as Error) }
    process.on('unhandledRejection', onReject)

    client.subscribe('t1', () => {})
    await new Promise((r) => setTimeout(r, 100))

    process.removeListener('unhandledRejection', onReject)
    expect(errors).toHaveLength(1)
    expect(errors[0]!.message).toBe('unexpected read error')
  })

  it('handles fetch failure gracefully', async () => {
    const fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'))
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    const received: unknown[] = []

    // Should not throw
    client.subscribe('t1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 50))

    expect(received).toHaveLength(0)
  })
})

describe('TaskcastServerClient re-exported from index', () => {
  it('is the same class exported from index', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ id: 'task-1', status: 'pending' }), {
        status: 201,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const client = new TaskcastServerClientFromIndex({ baseUrl: 'http://taskcast', fetch })
    const task = await client.createTask({})
    expect(task.id).toBe('task-1')
  })
})
