import { describe, it, expect } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

describe('CLI — startup scenarios', () => {
  it('memory mode: /health responds ok', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const app = createTaskcastApp({ engine, auth: { mode: 'none' } })

    const res = await app.request('/health')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body).toEqual({ ok: true })
  })

  it('auth jwt mode rejects unauthenticated requests', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const app = createTaskcastApp({
      engine,
      auth: { mode: 'jwt', jwt: { algorithm: 'HS256', secret: 'test-secret-key-for-hmac-256' } },
    })

    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })
    expect(res.status).toBe(401)
  })
})
