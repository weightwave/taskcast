import { describe, expect, it, vi } from 'vitest'
import { Hono } from 'hono'
import { HTTPException } from 'hono/http-exception'
import {
  createHttpFailureLogger,
  createTaskcastApp,
  parseLogLevel,
  sanitizeErrorMessage,
} from '../src/index.js'
import {
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  TaskEngine,
} from '@taskcast/core'
import type { HttpFailureLog } from '../src/index.js'

function collectingApp(
  route: (app: Hono) => void,
): { app: Hono; records: HttpFailureLog[] } {
  const records: HttpFailureLog[] = []
  const app = new Hono()
  app.use('*', createHttpFailureLogger({
    logLevel: 'info',
    logger: (record) => records.push(record),
  }))
  route(app)
  return { app, records }
}

describe('HTTP failure logging', () => {
  it('logs a thrown /tasks failure exactly once without changing the response', async () => {
    const { app, records } = collectingApp((router) => {
      router.get('/tasks', () => {
        throw new Error(
          'redis://admin:secret@redis.example.com:6379 broken pipe',
        )
      })
    })

    const response = await app.request('/tasks?access_token=do-not-log', {
      headers: { authorization: 'Bearer do-not-log' },
    })

    expect(response.status).toBe(500)
    expect(await response.text()).toBe('Internal Server Error')
    expect(records).toHaveLength(1)
    expect(records[0]).toMatchObject({
      level: 'error',
      event: 'http_request_failed',
      method: 'GET',
      path: '/tasks',
      status: 500,
      errorKind: 'store',
      error: 'redis://***@redis.example.com:6379 broken pipe',
    })
    expect(JSON.stringify(records[0])).not.toContain('access_token')
    expect(JSON.stringify(records[0])).not.toContain('Bearer')
    expect(JSON.stringify(records[0])).not.toContain('secret')
  })

  it('logs a manually returned 500 exactly once without invented details', async () => {
    const { app, records } = collectingApp((router) => {
      router.post('/manual', (c) => c.text('existing response', 500))
    })

    const response = await app.request('/manual?secret=query-secret', {
      method: 'POST',
      headers: {
        authorization: 'Bearer header-secret',
        'content-type': 'text/plain',
      },
      body: 'body-secret',
    })

    expect(response.status).toBe(500)
    expect(await response.text()).toBe('existing response')
    expect(records).toHaveLength(1)
    expect(records[0]).toMatchObject({
      method: 'POST',
      path: '/manual',
      status: 500,
    })
    expect(records[0]?.error).toBeUndefined()
    expect(records[0]?.errorKind).toBeUndefined()
    const serialized = JSON.stringify(records[0])
    expect(serialized).not.toContain('query-secret')
    expect(serialized).not.toContain('header-secret')
    expect(serialized).not.toContain('body-secret')
  })

  it('logs the upper 5xx boundary', async () => {
    const { app, records } = collectingApp((router) => {
      router.get('/upper-bound', () => new Response(null, { status: 599 }))
    })

    await app.request('/upper-bound')

    expect(records).toHaveLength(1)
    expect(records[0]?.status).toBe(599)
  })

  it('does not log 2xx, 3xx, or 4xx responses', async () => {
    const { app, records } = collectingApp((router) => {
      router.get('/ok', (c) => c.body(null, 200))
      router.get('/redirect', (c) => c.redirect('/ok'))
      router.get('/bad', (c) => c.body(null, 400))
      router.get('/missing', (c) => c.notFound())
    })

    await Promise.all([
      app.request('/ok'),
      app.request('/redirect'),
      app.request('/bad'),
      app.request('/missing'),
    ])
    expect(records).toEqual([])
  })

  it.each(['debug', 'info', 'warn', 'error'] as const)(
    'emits error records at the %s threshold',
    async (logLevel) => {
      const records: HttpFailureLog[] = []
      const app = new Hono()
      app.use('*', createHttpFailureLogger({
        logLevel,
        logger: (record) => records.push(record),
      }))
      app.get('/failure', (c) => c.body(null, 500))

      await app.request('/failure')

      expect(records).toHaveLength(1)
    },
  )

  it.each([
    ['/tasks/import', 'archive'],
    ['/tasks/task-1/archive', 'archive'],
    ['/tasks', 'store'],
    ['/tasks/task-1', 'store'],
    ['/events', 'store'],
    ['/workers/ws', 'store'],
    ['/other', 'internal'],
  ] as const)('classifies an error at %s as %s', async (path, errorKind) => {
    const { app, records } = collectingApp((router) => {
      router.get(path, () => {
        throw new Error('failure')
      })
    })

    await app.request(path)

    expect(records[0]?.errorKind).toBe(errorKind)
  })

  it('uses stderr JSON logging by default', async () => {
    const stderr = vi.spyOn(console, 'error').mockImplementation(() => {})
    const app = new Hono()
    app.use('*', createHttpFailureLogger())
    app.get('/failure', (c) => c.body(null, 500))

    await app.request('/failure')

    expect(stderr).toHaveBeenCalledTimes(1)
    expect(JSON.parse(String(stderr.mock.calls[0]?.[0]))).toMatchObject({
      event: 'http_request_failed',
      status: 500,
    })
    stderr.mockRestore()
  })

  it('truncates error text by Unicode scalar value', () => {
    const message = `${'😀'.repeat(2048)}tail`
    expect(Array.from(sanitizeErrorMessage(message) ?? '')).toHaveLength(2048)
    expect(sanitizeErrorMessage(message)).not.toContain('tail')
    expect(sanitizeErrorMessage('')).toBeUndefined()
  })

  it('parses documented log levels case-insensitively', () => {
    expect(parseLogLevel(undefined)).toBe('info')
    expect(parseLogLevel('DEBUG')).toBe('debug')
    expect(parseLogLevel('Info')).toBe('info')
    expect(parseLogLevel('warn')).toBe('warn')
    expect(parseLogLevel('error')).toBe('error')
    expect(() => parseLogLevel('trace')).toThrow(
      'invalid TASKCAST_LOG_LEVEL "trace"',
    )
  })

  it('is installed by createTaskcastApp', async () => {
    class BrokenStore extends MemoryShortTermStore {
      override async listTasks(): Promise<never> {
        throw new Error(
          'redis://admin:secret@redis.example.com:6379 broken pipe',
        )
      }
    }

    const stderr = vi.spyOn(console, 'error').mockImplementation(() => {})
    const records: HttpFailureLog[] = []
    const shortTermStore = new BrokenStore()
    const engine = new TaskEngine({
      shortTermStore,
      broadcast: new MemoryBroadcastProvider(),
    })
    const taskcast = createTaskcastApp({
      engine,
      shortTermStore,
      auth: { mode: 'none' },
      errorLogger: (record) => records.push(record),
    })
    taskcast.app.get('/teapot', () => {
      throw new HTTPException(418, { message: 'teapot' })
    })

    try {
      const response = await taskcast.app.request('/tasks')

      expect(response.status).toBe(500)
      expect(records).toHaveLength(1)
      expect(records[0]).toMatchObject({
        method: 'GET',
        path: '/tasks',
        status: 500,
        errorKind: 'store',
        error: 'redis://***@redis.example.com:6379 broken pipe',
      })

      const teapot = await taskcast.app.request('/teapot')
      expect(teapot.status).toBe(418)
      expect(await teapot.text()).toBe('teapot')
      expect(records).toHaveLength(1)
      expect(stderr).not.toHaveBeenCalled()
    } finally {
      stderr.mockRestore()
      taskcast.stop()
    }
  })
})
