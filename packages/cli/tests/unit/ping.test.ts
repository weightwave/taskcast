import { describe, it, expect } from 'vitest'
import { pingServer } from '../../src/commands/ping.js'
import type { PingResult } from '../../src/commands/ping.js'

describe('pingServer', () => {
  it('returns OK with latency on successful response', async () => {
    const mockFetch = async () => ({ ok: true, status: 200 }) as Response
    const result: PingResult = await pingServer('http://localhost:3721', mockFetch)
    expect(result.ok).toBe(true)
    expect(result.latencyMs).toBeTypeOf('number')
    expect(result.latencyMs).toBeGreaterThanOrEqual(0)
    expect(result.error).toBeUndefined()
  })

  it('returns FAIL on connection error', async () => {
    const mockFetch = async () => { throw new Error('ECONNREFUSED') }
    const result: PingResult = await pingServer('http://localhost:3721', mockFetch as typeof fetch)
    expect(result.ok).toBe(false)
    expect(result.error).toBe('ECONNREFUSED')
    expect(result.latencyMs).toBeUndefined()
  })

  it('returns FAIL on non-200 response', async () => {
    const mockFetch = async () => ({ ok: false, status: 503 }) as Response
    const result: PingResult = await pingServer('http://localhost:3721', mockFetch)
    expect(result.ok).toBe(false)
    expect(result.error).toBe('HTTP 503')
    expect(result.latencyMs).toBeUndefined()
  })

  it('calls the correct health endpoint URL', async () => {
    let calledUrl = ''
    const mockFetch = async (url: string | URL | Request) => {
      calledUrl = url as string
      return { ok: true, status: 200 } as Response
    }
    await pingServer('https://tc.example.com', mockFetch as typeof fetch)
    expect(calledUrl).toBe('https://tc.example.com/health')
  })
})
