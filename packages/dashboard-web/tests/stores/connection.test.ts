import { describe, it, expect, beforeEach, vi } from 'vitest'
import { useConnectionStore } from '@/stores/connection'

describe('useConnectionStore', () => {
  beforeEach(() => {
    // Reset store state before each test
    useConnectionStore.setState({
      baseUrl: '',
      jwt: null,
      connected: false,
      error: null,
    })
    vi.restoreAllMocks()
  })

  it('starts disconnected', () => {
    const state = useConnectionStore.getState()
    expect(state.connected).toBe(false)
    expect(state.baseUrl).toBe('')
    expect(state.jwt).toBe(null)
  })

  it('connects with admin token', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString()
      if (urlStr.endsWith('/health')) {
        return new Response('OK', { status: 200 })
      }
      if (urlStr.endsWith('/admin/token')) {
        return Response.json({ token: 'jwt-123' }, { status: 200 })
      }
      return new Response('Not Found', { status: 404 })
    })

    await useConnectionStore.getState().connect('http://localhost:3000', 'admin-secret')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(true)
    expect(state.baseUrl).toBe('http://localhost:3000')
    expect(state.jwt).toBe('jwt-123')
    expect(state.error).toBe(null)
  })

  it('connects without JWT when admin API returns 404', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString()
      if (urlStr.endsWith('/health')) {
        return new Response('OK', { status: 200 })
      }
      if (urlStr.endsWith('/admin/token')) {
        return new Response('Not Found', { status: 404 })
      }
      return new Response('Not Found', { status: 404 })
    })

    await useConnectionStore.getState().connect('http://localhost:3000', 'any-token')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(true)
    expect(state.baseUrl).toBe('http://localhost:3000')
    expect(state.jwt).toBe(null)
    expect(state.error).toBe(null)
  })

  it('sets error on invalid admin token (401)', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString()
      if (urlStr.endsWith('/health')) {
        return new Response('OK', { status: 200 })
      }
      if (urlStr.endsWith('/admin/token')) {
        return new Response('Unauthorized', { status: 401 })
      }
      return new Response('Not Found', { status: 404 })
    })

    await expect(
      useConnectionStore.getState().connect('http://localhost:3000', 'bad-token'),
    ).rejects.toThrow('Invalid admin token')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(false)
    expect(state.error).toBe('Invalid admin token')
  })

  it('sets error on unreachable server (fetch throws)', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('Network error'))

    await expect(
      useConnectionStore.getState().connect('http://localhost:9999', 'token'),
    ).rejects.toThrow('Network error')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(false)
    expect(state.error).toBe('Network error')
  })

  it('disconnects (resets jwt, connected=false)', async () => {
    // First connect
    useConnectionStore.setState({
      baseUrl: 'http://localhost:3000',
      jwt: 'jwt-123',
      connected: true,
      error: null,
    })

    useConnectionStore.getState().disconnect()

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(false)
    expect(state.jwt).toBe(null)
    expect(state.error).toBe(null)
  })

  it('setAutoConnect sets connected state', () => {
    useConnectionStore.getState().setAutoConnect('http://localhost:4000', 'auto-jwt')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(true)
    expect(state.baseUrl).toBe('http://localhost:4000')
    expect(state.jwt).toBe('auto-jwt')
    expect(state.error).toBe(null)
  })
})