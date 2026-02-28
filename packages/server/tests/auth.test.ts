import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { SignJWT } from 'jose'
import { createAuthMiddleware, checkScope } from '../src/auth.js'
import type { AuthConfig, AuthContext } from '../src/auth.js'

async function makeJwt(
  secret: Uint8Array,
  payload: Record<string, unknown>,
): Promise<string> {
  return new SignJWT(payload)
    .setProtectedHeader({ alg: 'HS256' })
    .setExpirationTime('1h')
    .sign(secret)
}

describe('auth middleware - mode: none', () => {
  it('allows all requests', async () => {
    const app = new Hono()
    app.use('*', createAuthMiddleware({ mode: 'none' }))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test')
    expect(res.status).toBe(200)
  })
})

describe('auth middleware - mode: jwt HS256', () => {
  it('rejects request with no token', async () => {
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test')
    expect(res.status).toBe(401)
  })

  it('rejects request with invalid token', async () => {
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test', {
      headers: { Authorization: 'Bearer invalid.token.here' },
    })
    expect(res.status).toBe(401)
  })

  it('accepts valid HS256 token and sets auth context', async () => {
    const secret = new TextEncoder().encode('test-secret-that-is-long-enough')
    const token = await makeJwt(secret, {
      taskIds: '*',
      scope: ['event:subscribe'],
    })
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => {
      const auth = c.get('auth')
      return c.json({ taskIds: auth.taskIds, scope: auth.scope })
    })
    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.taskIds).toBe('*')
    expect(body.scope).toContain('event:subscribe')
  })

  it('rejects expired token', async () => {
    const secret = new TextEncoder().encode('test-secret-that-is-long-enough')
    const token = await new SignJWT({ taskIds: '*', scope: ['*'] })
      .setProtectedHeader({ alg: 'HS256' })
      .setExpirationTime('-1s')
      .sign(secret)
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(401)
  })
})

describe('checkScope', () => {
  it('allows access when scope includes required permission', () => {
    const auth: AuthContext = { taskIds: '*', scope: ['event:subscribe'] }
    expect(checkScope(auth, 'event:subscribe')).toBe(true)
  })

  it('allows access when scope includes wildcard', () => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    expect(checkScope(auth, 'task:create')).toBe(true)
  })

  it('denies access when scope does not include required permission', () => {
    const auth: AuthContext = { taskIds: '*', scope: ['event:subscribe'] }
    expect(checkScope(auth, 'task:create')).toBe(false)
  })

  it('denies access when taskId not allowed', () => {
    const auth: AuthContext = { taskIds: ['task-abc'], scope: ['*'] }
    expect(checkScope(auth, 'event:subscribe', 'task-xyz')).toBe(false)
    expect(checkScope(auth, 'event:subscribe', 'task-abc')).toBe(true)
  })
})

describe('auth middleware - mode: custom', () => {
  it('calls custom middleware and sets auth context', async () => {
    const config: AuthConfig = {
      mode: 'custom',
      middleware: async (_req) => ({
        taskIds: ['task-1'],
        scope: ['event:subscribe' as const],
      }),
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => {
      const auth = c.get('auth')
      return c.json({ taskIds: auth.taskIds })
    })
    const res = await app.request('/test')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.taskIds).toEqual(['task-1'])
  })

  it('returns 401 when custom middleware returns null', async () => {
    const config: AuthConfig = {
      mode: 'custom',
      middleware: async (_req) => null,
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test')
    expect(res.status).toBe(401)
  })
})

describe('auth middleware - mode: jwt with issuer/audience', () => {
  it('accepts token with matching issuer and audience', async () => {
    const secret = new TextEncoder().encode('test-secret-that-is-long-enough')
    const token = await new SignJWT({ taskIds: '*', scope: ['*'] })
      .setProtectedHeader({ alg: 'HS256' })
      .setIssuer('my-issuer')
      .setAudience('my-audience')
      .setExpirationTime('1h')
      .sign(secret)
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: {
        algorithm: 'HS256',
        secret: 'test-secret-that-is-long-enough',
        issuer: 'my-issuer',
        audience: 'my-audience',
      },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(200)
  })

  it('rejects token with wrong issuer', async () => {
    const secret = new TextEncoder().encode('test-secret-that-is-long-enough')
    const token = await new SignJWT({ taskIds: '*', scope: ['*'] })
      .setProtectedHeader({ alg: 'HS256' })
      .setIssuer('wrong-issuer')
      .setExpirationTime('1h')
      .sign(secret)
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: {
        algorithm: 'HS256',
        secret: 'test-secret-that-is-long-enough',
        issuer: 'my-issuer',
      },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(401)
  })
})

describe('auth middleware - mode: jwt with sub claim', () => {
  it('sets sub on auth context when present in token', async () => {
    const secret = new TextEncoder().encode('test-secret-that-is-long-enough')
    const token = await new SignJWT({ taskIds: '*', scope: ['*'] })
      .setProtectedHeader({ alg: 'HS256' })
      .setSubject('user-123')
      .setExpirationTime('1h')
      .sign(secret)
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => {
      const auth = c.get('auth')
      return c.json({ sub: auth.sub })
    })
    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.sub).toBe('user-123')
  })
})

describe('auth middleware - fallthrough to 401', () => {
  it('returns 401 when mode is jwt but no jwt config provided', async () => {
    // mode is jwt but jwt property is missing - falls through to 401
    const config = { mode: 'jwt' as const }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))
    const res = await app.request('/test')
    expect(res.status).toBe(401)
  })
})
