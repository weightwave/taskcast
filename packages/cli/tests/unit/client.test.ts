import { describe, it, expect, vi } from 'vitest'
import { createClientFromNode, createClientFromNodeAsync } from '../../src/client.js'
import type { NodeEntry } from '../../src/node-config.js'

describe('createClientFromNode', () => {
  it('creates client with JWT token directly', () => {
    const node: NodeEntry = { url: 'https://tc.example.com', token: 'ey.jwt.tok', tokenType: 'jwt' }
    const client = createClientFromNode(node)
    expect(client).toBeDefined()
    // Verify it has the expected methods from TaskcastServerClient
    expect(typeof client.createTask).toBe('function')
    expect(typeof client.getTask).toBe('function')
  })

  it('creates client with no token', () => {
    const node: NodeEntry = { url: 'http://localhost:3721' }
    const client = createClientFromNode(node)
    expect(client).toBeDefined()
    expect(typeof client.createTask).toBe('function')
  })
})

describe('createClientFromNodeAsync', () => {
  it('exchanges admin token for JWT via POST /admin/token', async () => {
    const node: NodeEntry = { url: 'https://tc.example.com', token: 'admin_secret', tokenType: 'admin' }
    const mockJwt = 'ey.exchanged.jwt'

    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ token: mockJwt }),
    })

    const client = await createClientFromNodeAsync(node, mockFetch as unknown as typeof fetch)
    expect(client).toBeDefined()
    expect(typeof client.createTask).toBe('function')

    // Verify the admin token exchange request was made
    expect(mockFetch).toHaveBeenCalledOnce()
    const [url, init] = mockFetch.mock.calls[0]
    expect(url).toBe('https://tc.example.com/admin/token')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body)).toEqual({ adminToken: 'admin_secret' })
    expect(init.headers['Content-Type']).toBe('application/json')
  })

  it('throws on admin token exchange failure', async () => {
    const node: NodeEntry = { url: 'https://tc.example.com', token: 'bad_admin', tokenType: 'admin' }

    const mockFetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 401,
      json: async () => ({ error: 'Invalid admin token' }),
    })

    await expect(
      createClientFromNodeAsync(node, mockFetch as unknown as typeof fetch),
    ).rejects.toThrow('Invalid admin token')
  })

  it('returns client directly for JWT tokens without exchange', async () => {
    const node: NodeEntry = { url: 'https://tc.example.com', token: 'ey.jwt.tok', tokenType: 'jwt' }

    const mockFetch = vi.fn()
    const client = await createClientFromNodeAsync(node, mockFetch as unknown as typeof fetch)
    expect(client).toBeDefined()
    // No fetch call should have been made for JWT tokens
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('returns client directly for no-auth nodes without exchange', async () => {
    const node: NodeEntry = { url: 'http://localhost:3721' }

    const mockFetch = vi.fn()
    const client = await createClientFromNodeAsync(node, mockFetch as unknown as typeof fetch)
    expect(client).toBeDefined()
    expect(mockFetch).not.toHaveBeenCalled()
  })
})
