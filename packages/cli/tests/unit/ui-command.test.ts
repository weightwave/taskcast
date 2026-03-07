import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

// Mock dashboard-web
vi.mock('@taskcast/dashboard-web/dist-path', () => ({
  dashboardDistPath: '/fake/dashboard/dist',
}))

// Mock hono
const mockHonoGet = vi.fn()
const mockHonoApp = {
  get: mockHonoGet,
  fetch: vi.fn(),
}
vi.mock('hono', () => ({
  Hono: vi.fn().mockImplementation(() => mockHonoApp),
}))

// Mock @hono/node-server
const mockServe = vi.fn()
vi.mock('@hono/node-server', () => ({
  serve: (...args: unknown[]) => mockServe(...args),
}))

// Mock fs
const mockExistsSync = vi.fn()
const mockReadFileSync = vi.fn()
const mockStatSync = vi.fn()
vi.mock('fs', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    existsSync: (...args: unknown[]) => mockExistsSync(...args),
    readFileSync: (...args: unknown[]) => mockReadFileSync(...args),
    statSync: (...args: unknown[]) => mockStatSync(...args),
  }
})

// Mock path
const mockResolve = vi.fn().mockImplementation((...args: string[]) => args.join('/'))
vi.mock('path', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    join: (...args: string[]) => args.join('/'),
    extname: (p: string) => {
      const dot = p.lastIndexOf('.')
      return dot >= 0 ? p.slice(dot) : ''
    },
    resolve: (...args: string[]) => mockResolve(...args),
  }
})

// Mock fetch for admin token exchange
const originalFetch = globalThis.fetch

import { registerUiCommand } from '../../src/commands/ui.js'

describe('registerUiCommand', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation((() => {}) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.clearAllMocks()
  })

  afterEach(() => {
    exitSpy.mockRestore()
    logSpy.mockRestore()
    errorSpy.mockRestore()
    globalThis.fetch = originalFetch
  })

  it('starts dashboard server when dist exists', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui'])

    expect(mockServe).toHaveBeenCalled()
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Dashboard running'))
  })

  it('exits with 1 when dist does not exist', async () => {
    mockExistsSync.mockReturnValue(false)

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui'])

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('Dashboard not built'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exchanges admin token at startup', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ token: 'exchanged-jwt' }),
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui', '--admin-token', 'admin_secret'])

    expect(globalThis.fetch).toHaveBeenCalledWith(
      'http://localhost:3721/admin/token',
      expect.objectContaining({ method: 'POST' }),
    )
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Admin token provided'))
  })

  it('handles admin API not enabled (404) gracefully', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 404,
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui', '--admin-token', 'admin_secret'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Admin API not enabled'))
    expect(exitSpy).not.toHaveBeenCalledWith(1)
  })

  it('exits with 1 when admin token exchange fails with non-404 error', async () => {
    mockExistsSync.mockReturnValue(true)

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 500,
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui', '--admin-token', 'admin_secret'])

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('Failed to exchange admin token'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exits with 1 when server is unreachable', async () => {
    mockExistsSync.mockReturnValue(true)

    globalThis.fetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED')) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui', '--admin-token', 'admin_secret'])

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('Cannot reach server'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('registers api/config and static file routes', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui'])

    // Should have registered /api/config and a catch-all GET *
    expect(mockHonoGet).toHaveBeenCalledWith('/api/config', expect.any(Function))
    expect(mockHonoGet).toHaveBeenCalledWith('*', expect.any(Function))
  })

  it('uses dashboard alias', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'dashboard'])

    expect(mockServe).toHaveBeenCalled()
  })

  it('uses custom port', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui', '-p', '8080'])

    const serveCall = mockServe.mock.calls[0]
    expect(serveCall[0].port).toBe(8080)
  })

  it('/api/config returns baseUrl and token', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ token: 'my-jwt' }),
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui', '--admin-token', 'admin_secret', '-s', 'http://myserver:3721'])

    // Find the /api/config handler
    const apiConfigCall = mockHonoGet.mock.calls.find((c: unknown[]) => c[0] === '/api/config')
    expect(apiConfigCall).toBeDefined()
    const handler = apiConfigCall![1]

    // Invoke the handler with a mock context
    let jsonResult: unknown
    const mockCtx = {
      json: (data: unknown) => { jsonResult = data; return new Response() },
    }
    handler(mockCtx)

    expect(jsonResult).toEqual({
      baseUrl: 'http://myserver:3721',
      token: 'my-jwt',
    })
  })

  it('GET * serves static file when file exists', async () => {
    mockExistsSync.mockReturnValue(true)
    mockStatSync.mockReturnValue({ isFile: () => true })
    mockReadFileSync.mockReturnValue(Buffer.from('<html></html>'))
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui'])

    // Find the * handler
    const wildcardCall = mockHonoGet.mock.calls.find((c: unknown[]) => c[0] === '*')
    expect(wildcardCall).toBeDefined()
    const handler = wildcardCall![1]

    // Invoke with a mock context for /index.html
    let responseBody: unknown
    let responseStatus: number | undefined
    let responseHeaders: Record<string, string> | undefined
    const mockCtx = {
      req: { url: 'http://localhost:3722/index.html' },
      text: (t: string, s: number) => { responseBody = t; responseStatus = s; return new Response() },
      body: (b: unknown, s: number, h: Record<string, string>) => {
        responseBody = b
        responseStatus = s
        responseHeaders = h
        return new Response()
      },
    }
    handler(mockCtx)

    expect(responseStatus).toBe(200)
    expect(responseHeaders?.['Content-Type']).toBe('text/html')
  })

  it('GET * falls back to index.html for SPA routes', async () => {
    let callCount = 0
    mockExistsSync.mockImplementation((p: string) => {
      callCount++
      if (callCount <= 1) return true // first call: dist dir check
      if (p.includes('/fake/dashboard/dist/some-page')) return false // exact file not found
      return true // index.html exists
    })
    mockStatSync.mockReturnValue({ isFile: () => false })
    mockReadFileSync.mockReturnValue(Buffer.from('<html>SPA</html>'))
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui'])

    const wildcardCall = mockHonoGet.mock.calls.find((c: unknown[]) => c[0] === '*')
    const handler = wildcardCall![1]

    let responseStatus: number | undefined
    const mockCtx = {
      req: { url: 'http://localhost:3722/some-page' },
      text: (_t: string, s: number) => { responseStatus = s; return new Response() },
      body: (_b: unknown, s: number, _h: Record<string, string>) => {
        responseStatus = s
        return new Response()
      },
    }
    handler(mockCtx)

    expect(responseStatus).toBe(200)
  })

  it('GET * returns 404 for path traversal', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui'])

    const wildcardCall = mockHonoGet.mock.calls.find((c: unknown[]) => c[0] === '*')
    const handler = wildcardCall![1]

    // Make resolve return a path outside the dashboardDistPath
    mockResolve.mockReturnValueOnce('/etc/passwd')

    let responseBody: unknown
    let responseStatus: number | undefined
    const mockCtx = {
      req: { url: 'http://localhost:3722/etc/passwd' },
      text: (t: string, s: number) => {
        responseBody = t
        responseStatus = s
        return new Response()
      },
      body: (_b: unknown, s: number, _h: Record<string, string>) => {
        responseStatus = s
        return new Response()
      },
    }
    handler(mockCtx)

    expect(responseStatus).toBe(404)
    expect(responseBody).toBe('Not Found')
  })

  it('GET * returns 404 when index.html also missing', async () => {
    mockExistsSync.mockReturnValue(true)
    mockStatSync.mockReturnValue({ isFile: () => false })
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    // After initial dist check, when looking for the file and index.html
    let callIdx = 0
    mockExistsSync.mockImplementation(() => {
      callIdx++
      if (callIdx === 1) return true // dist dir exists
      return false // file not found, index.html not found
    })

    const program = new Command()
    program.exitOverride()
    registerUiCommand(program)

    await program.parseAsync(['node', 'test', 'ui'])

    const wildcardCall = mockHonoGet.mock.calls.find((c: unknown[]) => c[0] === '*')
    const handler = wildcardCall![1]

    let responseBody: unknown
    let responseStatus: number | undefined
    const mockCtx = {
      req: { url: 'http://localhost:3722/nonexistent.xyz' },
      text: (t: string, s: number) => {
        responseBody = t
        responseStatus = s
        return new Response()
      },
      body: (_b: unknown, s: number, _h: Record<string, string>) => {
        responseStatus = s
        return new Response()
      },
    }
    handler(mockCtx)

    expect(responseStatus).toBe(404)
    expect(responseBody).toBe('Not Found')
  })
})
