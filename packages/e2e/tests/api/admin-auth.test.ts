import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'

describe('Admin Auth Flow API', () => {
  let server: TestServer

  beforeAll(async () => {
    server = await startServer({
      auth: 'jwt',
      adminApi: true,
      adminToken: 'e2e-admin-token',
    })
  })

  afterAll(() => {
    server.close()
  })

  // ─── Token Exchange ───────────────────────────────────────────────────────

  it('exchanges admin token for JWT (200 with token + expiresAt)', async () => {
    const res = await fetch(`${server.baseUrl}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'e2e-admin-token' }),
    })

    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeDefined()
    expect(typeof body.token).toBe('string')
    expect(body.token.length).toBeGreaterThan(0)
    expect(body.expiresAt).toBeDefined()
    expect(typeof body.expiresAt).toBe('number')
  })

  it('rejects invalid admin token (401)', async () => {
    const res = await fetch(`${server.baseUrl}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'wrong-token' }),
    })

    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBeDefined()
  })

  // ─── JWT Auth ─────────────────────────────────────────────────────────────

  it('JWT grants access to protected endpoints (POST /tasks)', async () => {
    // Get a JWT first
    const tokenRes = await fetch(`${server.baseUrl}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'e2e-admin-token' }),
    })
    const { token } = await tokenRes.json()

    // Use JWT to create a task
    const res = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ type: 'auth-test' }),
    })

    expect(res.status).toBe(201)
    const task = await res.json()
    expect(task.id).toBeDefined()
    expect(task.type).toBe('auth-test')
  })

  it('rejects requests without Bearer token (401)', async () => {
    const res = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'no-auth' }),
    })

    expect(res.status).toBe(401)
    const body = await res.json()
    expect(body.error).toBeDefined()
  })

  it('admin endpoint bypasses JWT auth (works without Bearer)', async () => {
    // The admin/token endpoint should work without a Bearer token
    // because it's mounted BEFORE the auth middleware
    const res = await fetch(`${server.baseUrl}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'e2e-admin-token' }),
    })

    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeDefined()
  })

  it('custom scopes work (task:create scope grants create access)', async () => {
    // Request token with only task:create scope
    const tokenRes = await fetch(`${server.baseUrl}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        adminToken: 'e2e-admin-token',
        scopes: ['task:create'],
      }),
    })
    expect(tokenRes.status).toBe(200)
    const { token } = await tokenRes.json()

    // task:create should work for POST /tasks
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ type: 'scoped-test' }),
    })
    expect(createRes.status).toBe(201)

    // task:create should NOT grant event:subscribe (GET /tasks requires event:subscribe)
    const listRes = await fetch(`${server.baseUrl}/tasks`, {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(listRes.status).toBe(403)
  })
})