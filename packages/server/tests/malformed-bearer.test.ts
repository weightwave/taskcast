import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { createAuthMiddleware } from '../src/auth.js'
import type { AuthConfig } from '../src/auth.js'

function makeJwtApp() {
  const config: AuthConfig = {
    mode: 'jwt',
    jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
  }
  const app = new Hono()
  app.use('*', createAuthMiddleware(config))
  app.get('/test', (c) => c.json({ ok: true }))
  return app
}

describe('Malformed Bearer token — jwt mode', () => {
  it('returns 401 for "Bearer" with no token after space', async () => {
    const app = makeJwtApp()
    const res = await app.request('/test', {
      headers: { Authorization: 'Bearer ' },
    })
    // "Bearer " has nothing after the space, so token is empty string
    // jwtVerify should reject an empty string
    expect(res.status).toBe(401)
  })

  it('returns 401 for "Bearer" with no space at all', async () => {
    const app = makeJwtApp()
    const res = await app.request('/test', {
      headers: { Authorization: 'Bearer' },
    })
    // "Bearer" doesn't start with "Bearer " (note the missing trailing space)
    // So authHeader?.startsWith('Bearer ') returns false -> Missing Bearer token
    expect(res.status).toBe(401)
  })

  it('returns 401 for lowercase "bearer token"', async () => {
    const app = makeJwtApp()
    const res = await app.request('/test', {
      headers: { Authorization: 'bearer some-token-value' },
    })
    // "bearer" (lowercase) does not match startsWith('Bearer ')
    expect(res.status).toBe(401)
  })

  it('returns 401 for "Basic" auth scheme', async () => {
    const app = makeJwtApp()
    const res = await app.request('/test', {
      headers: { Authorization: 'Basic dXNlcjpwYXNz' },
    })
    // "Basic ..." does not match startsWith('Bearer ')
    expect(res.status).toBe(401)
  })

  it('returns 401 when no Authorization header at all', async () => {
    const app = makeJwtApp()
    const res = await app.request('/test')
    // No Authorization header -> authHeader is undefined
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toContain('Missing Bearer token')
  })

  it('returns 401 for garbled token', async () => {
    const app = makeJwtApp()
    const res = await app.request('/test', {
      headers: { Authorization: 'Bearer not.a.valid.jwt.at.all' },
    })
    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toContain('Invalid or expired token')
  })

  it('returns 401 for token with extra whitespace', async () => {
    const app = makeJwtApp()
    const res = await app.request('/test', {
      headers: { Authorization: 'Bearer   ' },
    })
    // Token will be "  " (whitespace), which is not a valid JWT
    expect(res.status).toBe(401)
  })
})
