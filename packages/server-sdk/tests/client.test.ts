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
