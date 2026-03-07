import { describe, it, expect } from 'vitest'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

describe('GET /health/detail', () => {
  it('returns 200 with ok, uptime, auth, and adapters fields', async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })

    const res = await app.request('/health/detail')
    expect(res.status).toBe(200)

    const body = await res.json()
    expect(body).toHaveProperty('ok', true)
    expect(body).toHaveProperty('uptime')
    expect(body).toHaveProperty('auth')
    expect(body).toHaveProperty('adapters')
  })

  it('uptime is a non-negative number', async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })

    const res = await app.request('/health/detail')
    const body = await res.json()

    expect(typeof body.uptime).toBe('number')
    expect(body.uptime).toBeGreaterThanOrEqual(0)
  })

  it('reports correct auth mode (none)', async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })

    const res = await app.request('/health/detail')
    const body = await res.json()

    expect(body.auth).toEqual({ mode: 'none' })
  })

  it('reports memory adapters by default', async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })

    const res = await app.request('/health/detail')
    const body = await res.json()

    expect(body.adapters.broadcast).toEqual({ provider: 'memory', status: 'ok' })
    expect(body.adapters.shortTermStore).toEqual({ provider: 'memory', status: 'ok' })
    expect(body.adapters.longTermStore).toBeUndefined()
  })

  it('includes longTermStore when config specifies it', async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    const { app } = createTaskcastApp({
      engine,
      auth: { mode: 'none' },
      config: {
        adapters: {
          broadcast: { provider: 'redis' },
          shortTermStore: { provider: 'redis' },
          longTermStore: { provider: 'postgres' },
        },
      },
    })

    const res = await app.request('/health/detail')
    const body = await res.json()

    expect(body.adapters.broadcast).toEqual({ provider: 'redis', status: 'ok' })
    expect(body.adapters.shortTermStore).toEqual({ provider: 'redis', status: 'ok' })
    expect(body.adapters.longTermStore).toEqual({ provider: 'postgres', status: 'ok' })
  })

  it('does not require authentication', async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    const { app } = createTaskcastApp({
      engine,
      auth: { mode: 'jwt', jwt: { secret: 'test-secret', algorithm: 'HS256' } },
    })

    // Request without any auth header should still succeed
    const res = await app.request('/health/detail')
    expect(res.status).toBe(200)

    const body = await res.json()
    expect(body.ok).toBe(true)
    expect(body.auth.mode).toBe('jwt')
  })
})
