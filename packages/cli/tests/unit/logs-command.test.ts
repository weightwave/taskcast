import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

// Mock NodeConfigManager
const mockGet = vi.fn()
const mockGetCurrent = vi.fn()

vi.mock('../../src/node-config.js', () => ({
  NodeConfigManager: vi.fn().mockImplementation(() => ({
    get: mockGet,
    getCurrent: mockGetCurrent,
  })),
}))

import { registerLogsCommand, registerTailCommand } from '../../src/commands/logs.js'

const originalFetch = globalThis.fetch

function makeSSEResponse(chunks: string[]): Response {
  let chunkIndex = 0
  const encoder = new TextEncoder()

  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (chunkIndex < chunks.length) {
        controller.enqueue(encoder.encode(chunks[chunkIndex]))
        chunkIndex++
      } else {
        controller.close()
      }
    },
  })

  return {
    ok: true,
    status: 200,
    body: stream,
  } as unknown as Response
}

// Helper: make process.exit throw so execution stops (matching real behavior)
class ExitError extends Error {
  code: number
  constructor(code: number) {
    super(`process.exit(${code})`)
    this.code = code
  }
}

describe('registerLogsCommand', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
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

  it('streams events from a task using current node', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721', token: 'my-jwt', tokenType: 'jwt' })

    const sseData = 'event: taskcast.event\ndata: {"type":"llm.delta","level":"info","timestamp":1000,"data":{"delta":"hi"}}\n\nevent: taskcast.done\ndata: {"reason":"completed"}\n\n'

    globalThis.fetch = vi.fn().mockResolvedValue(makeSSEResponse([sseData])) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerLogsCommand(program)

    // exit(0) will be called on done event, which throws ExitError
    try {
      await program.parseAsync(['node', 'test', 'logs', 'task-123'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 0)) throw e
    }

    // Should have logged the event and the done event
    expect(logSpy).toHaveBeenCalled()
    expect(exitSpy).toHaveBeenCalledWith(0)
  })

  it('passes type and level filter query params', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    let capturedUrl = ''
    globalThis.fetch = vi.fn().mockImplementation(async (url: string) => {
      capturedUrl = url
      return makeSSEResponse([])
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerLogsCommand(program)

    await program.parseAsync(['node', 'test', 'logs', 'task-123', '--types', 'llm.*', '--levels', 'info,warn'])

    expect(capturedUrl).toContain('types=llm.*')
    expect(capturedUrl).toContain('levels=info%2Cwarn')
  })

  it('uses --node option to look up specific node', async () => {
    mockGet.mockReturnValue({ url: 'https://prod.example.com' })

    globalThis.fetch = vi.fn().mockResolvedValue(makeSSEResponse([])) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerLogsCommand(program)

    await program.parseAsync(['node', 'test', 'logs', 'task-123', '--node', 'prod'])

    expect(mockGet).toHaveBeenCalledWith('prod')
  })

  it('exits with 1 when --node not found', async () => {
    mockGet.mockReturnValue(undefined)

    const program = new Command()
    program.exitOverride()
    registerLogsCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'logs', 'task-123', '--node', 'ghost'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Node "ghost" not found')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exits with 1 on fetch error', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    globalThis.fetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED')) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerLogsCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'logs', 'task-123'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Error: ECONNREFUSED')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exchanges admin token before streaming', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721', token: 'admin_secret', tokenType: 'admin' })

    let callIndex = 0
    globalThis.fetch = vi.fn().mockImplementation(async (url: string) => {
      callIndex++
      if (callIndex === 1) {
        // Admin token exchange
        expect(url).toBe('http://localhost:3721/admin/token')
        return { ok: true, json: async () => ({ token: 'exchanged-jwt' }) } as Response
      }
      // SSE request
      return makeSSEResponse([])
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerLogsCommand(program)

    await program.parseAsync(['node', 'test', 'logs', 'task-123'])

    expect(callIndex).toBe(2)
  })

  it('handles admin token exchange failure', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721', token: 'bad_admin', tokenType: 'admin' })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 401,
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerLogsCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'logs', 'task-123'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('Error:'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })
})

describe('registerTailCommand', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
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

  it('streams events from all tasks using current node', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    const sseData = 'event: taskcast.event\ndata: {"type":"llm.delta","level":"info","timestamp":1000,"data":{"delta":"hi"},"taskId":"01JXX123"}\n\n'

    globalThis.fetch = vi.fn().mockResolvedValue(makeSSEResponse([sseData])) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerTailCommand(program)

    await program.parseAsync(['node', 'test', 'tail'])

    expect(logSpy).toHaveBeenCalled()
  })

  it('passes filter query params', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    let capturedUrl = ''
    globalThis.fetch = vi.fn().mockImplementation(async (url: string) => {
      capturedUrl = url
      return makeSSEResponse([])
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerTailCommand(program)

    await program.parseAsync(['node', 'test', 'tail', '--types', 'llm.*', '--levels', 'error'])

    expect(capturedUrl).toContain('/events')
    expect(capturedUrl).toContain('types=llm.*')
    expect(capturedUrl).toContain('levels=error')
  })

  it('uses --node option to look up specific node', async () => {
    mockGet.mockReturnValue({ url: 'https://prod.example.com' })

    globalThis.fetch = vi.fn().mockResolvedValue(makeSSEResponse([])) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerTailCommand(program)

    await program.parseAsync(['node', 'test', 'tail', '--node', 'prod'])

    expect(mockGet).toHaveBeenCalledWith('prod')
  })

  it('exits with 1 when --node not found', async () => {
    mockGet.mockReturnValue(undefined)

    const program = new Command()
    program.exitOverride()
    registerTailCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'tail', '--node', 'ghost'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Node "ghost" not found')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exits with 1 on fetch error', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    globalThis.fetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED')) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerTailCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'tail'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Error: ECONNREFUSED')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })
})
