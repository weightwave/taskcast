import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { jwtVerify } from 'jose'
import { createAdminRouter } from '../../src/routes/admin.js'
import type { AdminRouteOptions } from '../../src/routes/admin.js'
import type { TaskcastConfig } from '@taskcast/core'
import type { AuthConfig } from '../../src/auth.js'

const TEST_SECRET = 'test-secret-that-is-long-enough-for-HS256'

function makeApp(overrides: {
  config?: Partial<TaskcastConfig>
  auth?: AuthConfig
} = {}) {
  const config: TaskcastConfig = {
    adminApi: true,
    adminToken: 'my-admin-token',
    ...overrides.config,
  }
  const auth: AuthConfig = overrides.auth ?? { mode: 'none' }
  const opts: AdminRouteOptions = { config, auth }
  const app = new Hono()
  app.route('/admin', createAdminRouter(opts))
  return { app, config }
}

function post(app: Hono, body: unknown) {
  return app.request('/admin/token', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
}

// ─── Happy path ─────────────────────────────────────────────────────────────

describe('POST /admin/token — happy path', () => {
  it('returns 200 with JWT when auth mode is jwt and admin token is valid', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, { adminToken: 'my-admin-token' })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeTruthy()
    expect(typeof body.token).toBe('string')
    expect(body.token.length).toBeGreaterThan(0)
    expect(typeof body.expiresAt).toBe('number')
    expect(body.expiresAt).toBeGreaterThan(Math.floor(Date.now() / 1000))
  })

  it('returns 200 with placeholder token when auth mode is none', async () => {
    const { app } = makeApp({ auth: { mode: 'none' } })
    const res = await post(app, { adminToken: 'my-admin-token' })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBe('')
    expect(typeof body.expiresAt).toBe('number')
  })
})

// ─── JWT claims verification ────────────────────────────────────────────────

describe('POST /admin/token — JWT claims', () => {
  it('JWT contains correct default claims (sub, scope, taskIds, exp)', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, { adminToken: 'my-admin-token' })
    const body = await res.json()

    const secret = new TextEncoder().encode(TEST_SECRET)
    const { payload } = await jwtVerify(body.token, secret)
    expect(payload.sub).toBe('admin')
    expect(payload['scope']).toEqual(['*'])
    expect(payload['taskIds']).toBe('*')
    expect(payload.exp).toBe(body.expiresAt)
    expect(payload.iat).toBeDefined()
  })

  it('JWT uses custom scopes when requested', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, {
      adminToken: 'my-admin-token',
      scopes: ['task:create', 'event:subscribe'],
    })
    const body = await res.json()

    const secret = new TextEncoder().encode(TEST_SECRET)
    const { payload } = await jwtVerify(body.token, secret)
    expect(payload['scope']).toEqual(['task:create', 'event:subscribe'])
  })

  it('JWT uses custom expiresIn when requested', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const beforeTime = Math.floor(Date.now() / 1000)
    const res = await post(app, {
      adminToken: 'my-admin-token',
      expiresIn: 3600, // 1 hour
    })
    const body = await res.json()

    // expiresAt should be approximately now + 3600, within 5 seconds tolerance
    expect(body.expiresAt).toBeGreaterThanOrEqual(beforeTime + 3600)
    expect(body.expiresAt).toBeLessThanOrEqual(beforeTime + 3600 + 5)

    const secret = new TextEncoder().encode(TEST_SECRET)
    const { payload } = await jwtVerify(body.token, secret)
    expect(payload.exp).toBe(body.expiresAt)
  })

  it('default expiresIn is 86400 (24h)', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const beforeTime = Math.floor(Date.now() / 1000)
    const res = await post(app, { adminToken: 'my-admin-token' })
    const body = await res.json()

    expect(body.expiresAt).toBeGreaterThanOrEqual(beforeTime + 86400)
    expect(body.expiresAt).toBeLessThanOrEqual(beforeTime + 86400 + 5)
  })
})

// ─── Invalid admin token ────────────────────────────────────────────────────

describe('POST /admin/token — invalid admin token', () => {
  it('returns 401 for wrong admin token', async () => {
    const { app } = makeApp()
    const res = await post(app, { adminToken: 'wrong-token' })
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBe('Invalid admin token')
  })

  it('returns 401 for missing adminToken field', async () => {
    const { app } = makeApp()
    const res = await post(app, {})
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBe('Invalid admin token')
  })

  it('returns 401 for empty string admin token', async () => {
    const { app } = makeApp()
    const res = await post(app, { adminToken: '' })
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBe('Invalid admin token')
  })

  it('returns 401 for non-string admin token (number)', async () => {
    const { app } = makeApp()
    const res = await post(app, { adminToken: 12345 })
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBe('Invalid admin token')
  })

  it('returns 401 for null admin token', async () => {
    const { app } = makeApp()
    const res = await post(app, { adminToken: null })
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBe('Invalid admin token')
  })
})

// ─── Admin API disabled ─────────────────────────────────────────────────────

describe('POST /admin/token — adminApi disabled', () => {
  it('returns 404 when adminApi is false', async () => {
    const { app } = makeApp({ config: { adminApi: false } })
    const res = await post(app, { adminToken: 'my-admin-token' })
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe('Not found')
  })

  it('returns 404 when adminApi is not set (default)', async () => {
    const { app } = makeApp({ config: { adminApi: undefined, adminToken: undefined } })
    const res = await post(app, { adminToken: 'my-admin-token' })
    expect(res.status).toBe(404)
  })
})

// ─── Auth mode interactions ─────────────────────────────────────────────────

describe('POST /admin/token — auth mode interactions', () => {
  it('auth mode "none" + valid admin token: validates token and returns placeholder', async () => {
    const { app } = makeApp({ auth: { mode: 'none' } })
    const res = await post(app, { adminToken: 'my-admin-token' })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBe('')
    expect(typeof body.expiresAt).toBe('number')
  })

  it('auth mode "none" + invalid admin token: still rejects', async () => {
    const { app } = makeApp({ auth: { mode: 'none' } })
    const res = await post(app, { adminToken: 'wrong-token' })
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBe('Invalid admin token')
  })

  it('auth mode "jwt" + valid admin token: returns real JWT', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, { adminToken: 'my-admin-token' })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeTruthy()
    expect(body.token.split('.')).toHaveLength(3) // JWT has 3 parts
  })

  it('auth mode "jwt" + invalid admin token: rejects with 401', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, { adminToken: 'wrong-token' })
    expect(res.status).toBe(401)
  })

  it('auth mode "custom" + valid admin token: returns placeholder (no jwt config)', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'custom',
        middleware: async () => null,
      },
    })
    const res = await post(app, { adminToken: 'my-admin-token' })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBe('')
  })
})

// ─── Edge cases ─────────────────────────────────────────────────────────────

describe('POST /admin/token — edge cases', () => {
  it('auto-generated ULID admin token works the same as configured token', async () => {
    // Simulate an auto-generated ULID token
    const ulidToken = '01HZQX5KBFN4RGWT8PVCS3DXEM'
    const { app } = makeApp({
      config: { adminApi: true, adminToken: ulidToken },
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, { adminToken: ulidToken })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeTruthy()
    expect(body.token.split('.')).toHaveLength(3)
  })

  it('invalid JSON body returns 400', async () => {
    const { app } = makeApp()
    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: 'not valid json{{{',
    })
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBe('Invalid request body')
  })

  it('default scopes is ["*"] when not specified', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, { adminToken: 'my-admin-token' })
    const body = await res.json()

    const secret = new TextEncoder().encode(TEST_SECRET)
    const { payload } = await jwtVerify(body.token, secret)
    expect(payload['scope']).toEqual(['*'])
  })

  it('non-array scopes defaults to ["*"]', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const res = await post(app, {
      adminToken: 'my-admin-token',
      scopes: 'not-an-array',
    })
    const body = await res.json()

    const secret = new TextEncoder().encode(TEST_SECRET)
    const { payload } = await jwtVerify(body.token, secret)
    expect(payload['scope']).toEqual(['*'])
  })

  it('negative expiresIn falls back to default 86400', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const beforeTime = Math.floor(Date.now() / 1000)
    const res = await post(app, {
      adminToken: 'my-admin-token',
      expiresIn: -100,
    })
    const body = await res.json()
    expect(body.expiresAt).toBeGreaterThanOrEqual(beforeTime + 86400)
    expect(body.expiresAt).toBeLessThanOrEqual(beforeTime + 86400 + 5)
  })

  it('zero expiresIn falls back to default 86400', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const beforeTime = Math.floor(Date.now() / 1000)
    const res = await post(app, {
      adminToken: 'my-admin-token',
      expiresIn: 0,
    })
    const body = await res.json()
    expect(body.expiresAt).toBeGreaterThanOrEqual(beforeTime + 86400)
    expect(body.expiresAt).toBeLessThanOrEqual(beforeTime + 86400 + 5)
  })

  it('non-number expiresIn falls back to default 86400', async () => {
    const { app } = makeApp({
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
    })
    const beforeTime = Math.floor(Date.now() / 1000)
    const res = await post(app, {
      adminToken: 'my-admin-token',
      expiresIn: 'one-hour',
    })
    const body = await res.json()
    expect(body.expiresAt).toBeGreaterThanOrEqual(beforeTime + 86400)
    expect(body.expiresAt).toBeLessThanOrEqual(beforeTime + 86400 + 5)
  })
})

// ─── Integration with createTaskcastApp ─────────────────────────────────────

describe('POST /admin/token — via createTaskcastApp', () => {
  it('admin route bypasses auth middleware', async () => {
    // Import createTaskcastApp to verify it mounts admin before auth
    const { createTaskcastApp } = await import('../../src/index.js')
    const { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } = await import('@taskcast/core')

    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const app = createTaskcastApp({
      engine,
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
      config: {
        adminApi: true,
        adminToken: 'secret-admin-token',
        auth: { mode: 'jwt', jwt: { algorithm: 'HS256', secret: TEST_SECRET } },
      },
    })

    // Admin route should work WITHOUT a Bearer token
    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'secret-admin-token' }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeTruthy()
  })

  it('other routes still require auth when jwt mode is on', async () => {
    const { createTaskcastApp } = await import('../../src/index.js')
    const { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } = await import('@taskcast/core')

    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const app = createTaskcastApp({
      engine,
      auth: {
        mode: 'jwt',
        jwt: { algorithm: 'HS256', secret: TEST_SECRET },
      },
      config: {
        adminApi: true,
        adminToken: 'secret-admin-token',
      },
    })

    // Tasks endpoint should require auth
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })
    expect(res.status).toBe(401)
  })

  it('admin route not mounted when config is not provided', async () => {
    const { createTaskcastApp } = await import('../../src/index.js')
    const { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } = await import('@taskcast/core')

    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const app = createTaskcastApp({ engine })

    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'anything' }),
    })
    // Should get 404 because the route is not mounted at all
    expect(res.status).toBe(404)
  })
})
