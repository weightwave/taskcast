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

import { registerDoctorCommand } from '../../src/commands/doctor.js'

// The fetch used by runDoctor is globalThis.fetch; we mock it
const originalFetch = globalThis.fetch

describe('registerDoctorCommand', () => {
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

  it('runs doctor against current node and logs result', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({
        ok: true,
        uptime: 42,
        auth: { mode: 'none' },
        adapters: {
          broadcast: { provider: 'memory', status: 'ok' },
          shortTermStore: { provider: 'memory', status: 'ok' },
        },
      }),
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerDoctorCommand(program)

    await program.parseAsync(['node', 'test', 'doctor'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Server:    OK'))
    expect(exitSpy).not.toHaveBeenCalled()
  })

  it('exits with 1 when server is unreachable', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    globalThis.fetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED')) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerDoctorCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'doctor'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Server:    FAIL'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('uses --node option to look up a specific node', async () => {
    mockGet.mockReturnValue({ url: 'https://prod.example.com' })

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({
        ok: true,
        uptime: 100,
        auth: { mode: 'none' },
        adapters: {
          broadcast: { provider: 'redis', status: 'ok' },
          shortTermStore: { provider: 'redis', status: 'ok' },
        },
      }),
    }) as unknown as typeof fetch

    const program = new Command()
    program.exitOverride()
    registerDoctorCommand(program)

    await program.parseAsync(['node', 'test', 'doctor', '--node', 'prod'])

    expect(mockGet).toHaveBeenCalledWith('prod')
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Server:    OK'))
  })

  it('exits with 1 when --node not found', async () => {
    mockGet.mockReturnValue(undefined)

    const program = new Command()
    program.exitOverride()
    registerDoctorCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'doctor', '--node', 'ghost'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Node "ghost" not found')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })
})
