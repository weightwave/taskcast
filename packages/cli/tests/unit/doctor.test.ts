import { describe, it, expect } from 'vitest'
import { runDoctor, formatDoctorResult } from '../../src/commands/doctor.js'
import type { DoctorResult } from '../../src/commands/doctor.js'
import type { NodeEntry } from '../../src/node-config.js'

function mockFetch(body: unknown, status = 200): typeof fetch {
  return (() =>
    Promise.resolve({
      ok: status >= 200 && status < 300,
      status,
      json: () => Promise.resolve(body),
    })) as unknown as typeof fetch
}

function throwingFetch(error: Error): typeof fetch {
  return (() => Promise.reject(error)) as unknown as typeof fetch
}

describe('runDoctor', () => {
  const baseNode: NodeEntry = { url: 'http://localhost:3721' }

  it('reports all OK when server is healthy', async () => {
    const body = {
      ok: true,
      uptime: 120,
      auth: { mode: 'none' },
      adapters: {
        broadcast: { provider: 'memory', status: 'ok' },
        shortTermStore: { provider: 'memory', status: 'ok' },
      },
    }
    const result = await runDoctor(baseNode, mockFetch(body))

    expect(result.server.ok).toBe(true)
    expect(result.server.url).toBe('http://localhost:3721')
    expect(result.server.uptime).toBe(120)
    expect(result.auth.status).toBe('ok')
    expect(result.auth.mode).toBe('none')
    expect(result.auth.message).toBeUndefined()
    expect(result.adapters.broadcast).toEqual({ provider: 'memory', status: 'ok' })
    expect(result.adapters.shortTermStore).toEqual({ provider: 'memory', status: 'ok' })
    expect(result.adapters.longTermStore).toBeUndefined()
  })

  it('reports FAIL when server is unreachable', async () => {
    const result = await runDoctor(
      baseNode,
      throwingFetch(new Error('fetch failed: ECONNREFUSED')),
    )

    expect(result.server.ok).toBe(false)
    expect(result.server.url).toBe('http://localhost:3721')
    expect(result.server.error).toBe('fetch failed: ECONNREFUSED')
    expect(result.auth.status).toBe('warn')
    expect(result.adapters).toEqual({})
  })

  it('reports WARN for auth when no token but server uses JWT', async () => {
    const body = {
      ok: true,
      uptime: 60,
      auth: { mode: 'jwt' },
      adapters: {
        broadcast: { provider: 'redis', status: 'ok' },
        shortTermStore: { provider: 'redis', status: 'ok' },
      },
    }
    const nodeWithoutToken: NodeEntry = { url: 'http://localhost:3721' }
    const result = await runDoctor(nodeWithoutToken, mockFetch(body))

    expect(result.auth.status).toBe('warn')
    expect(result.auth.mode).toBe('jwt')
    expect(result.auth.message).toBe('no token configured for this node')
  })

  it('reports OK for auth when node has a token and server uses JWT', async () => {
    const body = {
      ok: true,
      uptime: 60,
      auth: { mode: 'jwt' },
      adapters: {
        broadcast: { provider: 'memory', status: 'ok' },
        shortTermStore: { provider: 'memory', status: 'ok' },
      },
    }
    const nodeWithToken: NodeEntry = {
      url: 'http://localhost:3721',
      token: 'ey...',
      tokenType: 'jwt',
    }
    const result = await runDoctor(nodeWithToken, mockFetch(body))

    expect(result.auth.status).toBe('ok')
    expect(result.auth.mode).toBe('jwt')
    expect(result.auth.message).toBeUndefined()
  })

  it('includes longTermStore when present', async () => {
    const body = {
      ok: true,
      uptime: 300,
      auth: { mode: 'none' },
      adapters: {
        broadcast: { provider: 'redis', status: 'ok' },
        shortTermStore: { provider: 'redis', status: 'ok' },
        longTermStore: { provider: 'postgres', status: 'ok' },
      },
    }
    const result = await runDoctor(baseNode, mockFetch(body))

    expect(result.adapters.longTermStore).toEqual({ provider: 'postgres', status: 'ok' })
  })

  it('reports FAIL when server returns non-OK HTTP status', async () => {
    const result = await runDoctor(baseNode, mockFetch({}, 500))

    expect(result.server.ok).toBe(false)
    expect(result.server.error).toBe('HTTP 500')
    expect(result.auth.status).toBe('warn')
    expect(result.adapters).toEqual({})
  })
})

describe('formatDoctorResult', () => {
  it('formats all-OK result', () => {
    const result: DoctorResult = {
      server: { ok: true, url: 'http://localhost:3721', uptime: 120 },
      auth: { status: 'ok', mode: 'none' },
      adapters: {
        broadcast: { provider: 'memory', status: 'ok' },
        shortTermStore: { provider: 'memory', status: 'ok' },
      },
    }
    const output = formatDoctorResult(result)

    expect(output).toContain('Server:    OK  taskcast at http://localhost:3721 (uptime: 120s)')
    expect(output).toContain('Auth:      OK  none')
    expect(output).toContain('Broadcast: OK  memory')
    expect(output).toContain('ShortTerm: OK  memory')
    expect(output).toContain('LongTerm:  SKIP  not configured')
  })

  it('formats FAIL server result', () => {
    const result: DoctorResult = {
      server: { ok: false, url: 'http://localhost:3721', error: 'ECONNREFUSED' },
      auth: { status: 'warn' },
      adapters: {},
    }
    const output = formatDoctorResult(result)

    expect(output).toContain('Server:    FAIL  cannot reach http://localhost:3721: ECONNREFUSED')
    expect(output).toContain('Auth:      WARN')
  })

  it('formats auth WARN with message', () => {
    const result: DoctorResult = {
      server: { ok: true, url: 'http://localhost:3721', uptime: 60 },
      auth: { status: 'warn', mode: 'jwt', message: 'no token configured for this node' },
      adapters: {
        broadcast: { provider: 'memory', status: 'ok' },
        shortTermStore: { provider: 'memory', status: 'ok' },
      },
    }
    const output = formatDoctorResult(result)

    expect(output).toContain('Auth:      WARN  no token configured for this node')
  })

  it('includes longTermStore when present', () => {
    const result: DoctorResult = {
      server: { ok: true, url: 'http://localhost:3721', uptime: 300 },
      auth: { status: 'ok', mode: 'none' },
      adapters: {
        broadcast: { provider: 'redis', status: 'ok' },
        shortTermStore: { provider: 'redis', status: 'ok' },
        longTermStore: { provider: 'postgres', status: 'ok' },
      },
    }
    const output = formatDoctorResult(result)

    expect(output).toContain('Broadcast: OK  redis')
    expect(output).toContain('ShortTerm: OK  redis')
    expect(output).toContain('LongTerm:  OK  postgres')
    expect(output).not.toContain('SKIP')
  })

  it('shows SKIP for longTermStore when not configured', () => {
    const result: DoctorResult = {
      server: { ok: true, url: 'http://localhost:3721', uptime: 10 },
      auth: { status: 'ok', mode: 'none' },
      adapters: {
        broadcast: { provider: 'memory', status: 'ok' },
        shortTermStore: { provider: 'memory', status: 'ok' },
      },
    }
    const output = formatDoctorResult(result)

    expect(output).toContain('LongTerm:  SKIP  not configured')
  })

  it('omits uptime when not provided', () => {
    const result: DoctorResult = {
      server: { ok: true, url: 'http://localhost:3721' },
      auth: { status: 'ok', mode: 'none' },
      adapters: {
        broadcast: { provider: 'memory', status: 'ok' },
        shortTermStore: { provider: 'memory', status: 'ok' },
      },
    }
    const output = formatDoctorResult(result)

    expect(output).toContain('Server:    OK  taskcast at http://localhost:3721')
    expect(output).not.toContain('uptime')
  })
})
