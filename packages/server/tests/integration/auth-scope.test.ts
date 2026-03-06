import { describe, it, expect } from 'vitest'
import { SignJWT } from 'jose'
import { createTestServer } from '../helpers/test-server.js'
import type { AuthConfig } from '../../src/auth.js'

const SECRET = 'test-secret-that-is-long-enough-for-hs256'
const SECRET_KEY = new TextEncoder().encode(SECRET)

const AUTH_CONFIG: AuthConfig = {
  mode: 'jwt',
  jwt: { algorithm: 'HS256', secret: SECRET },
}

async function makeToken(payload: Record<string, unknown>): Promise<string> {
  return new SignJWT(payload)
    .setProtectedHeader({ alg: 'HS256' })
    .setExpirationTime('1h')
    .sign(SECRET_KEY)
}

describe('Server integration — auth scope enforcement', () => {
  it('no token → 401', async () => {
    const { app } = createTestServer({ auth: AUTH_CONFIG })

    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })
    expect(res.status).toBe(401)
  })

  it('token with wrong scope → 403 on task create', async () => {
    const { app } = createTestServer({ auth: AUTH_CONFIG })
    const token = await makeToken({ taskIds: '*', scope: ['event:subscribe'] })

    const res = await app.request('/tasks', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ type: 'test' }),
    })
    expect(res.status).toBe(403)
  })

  it('token with taskIds restriction → 403 on other task', async () => {
    const { app, engine } = createTestServer({ auth: AUTH_CONFIG })

    // Create task using engine directly (bypasses auth)
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')

    // Token only allows access to 'other-task-id'
    const token = await makeToken({
      taskIds: ['other-task-id'],
      scope: ['event:publish'],
    })

    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ type: 'evt', level: 'info', data: null }),
    })
    expect(res.status).toBe(403)
  })

  it('token with task:create + event:publish → create and publish succeeds', async () => {
    const { app } = createTestServer({ auth: AUTH_CONFIG })
    const token = await makeToken({
      taskIds: '*',
      scope: ['task:create', 'task:manage', 'event:publish'],
    })
    const headers = {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    }

    // Create task
    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers,
      body: JSON.stringify({ type: 'test' }),
    })
    expect(createRes.status).toBe(201)
    const task = await createRes.json()

    // Transition to running
    const runRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers,
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)

    // Publish event
    const evtRes = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers,
      body: JSON.stringify({ type: 'chunk', level: 'info', data: { n: 1 } }),
    })
    expect(evtRes.status).toBe(201)
  })

  it('health endpoint is accessible without auth', async () => {
    const { app } = createTestServer({ auth: AUTH_CONFIG })

    const res = await app.request('/health')
    expect(res.status).toBe(200)
  })
})
