import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

class ExitError extends Error {
  code: number
  constructor(code: number) {
    super(`process.exit(${code})`)
    this.code = code
  }
}

// Mock NodeConfigManager
const mockGet = vi.fn()
const mockGetCurrent = vi.fn()

vi.mock('../../src/node-config.js', () => ({
  NodeConfigManager: vi.fn().mockImplementation(() => ({
    get: mockGet,
    getCurrent: mockGetCurrent,
  })),
}))

import { registerPingCommand } from '../../src/commands/ping.js'

const originalFetch = globalThis.fetch

describe('registerPingCommand', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  afterEach(() => {
    exitSpy.mockRestore()
    logSpy.mockRestore()
    errorSpy.mockRestore()
    globalThis.fetch = originalFetch
    vi.clearAllMocks()
  })

  it('pings current node and logs OK', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerPingCommand(program)

    await program.parseAsync(['node', 'test', 'ping'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('OK'))
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('http://localhost:3721'))
    expect(exitSpy).not.toHaveBeenCalled()
  })

  it('pings and exits 1 on failure', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    globalThis.fetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED')) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerPingCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'ping'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('FAIL'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('uses --node option to look up a specific node', async () => {
    mockGet.mockReturnValue({ url: 'https://prod.example.com' })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerPingCommand(program)

    await program.parseAsync(['node', 'test', 'ping', '--node', 'prod'])

    expect(mockGet).toHaveBeenCalledWith('prod')
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('OK'))
  })

  it('exits with 1 when --node not found', async () => {
    mockGet.mockReturnValue(undefined)

    const program = new Command()
    program.exitOverride()
    registerPingCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'ping', '--node', 'ghost'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Node "ghost" not found')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })
})
