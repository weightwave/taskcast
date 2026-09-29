import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

// Mock filesystem access
const mockExistsSync = vi.fn()
vi.mock('fs', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    existsSync: (...args: unknown[]) => mockExistsSync(...args),
  }
})

// Mock @hono/zod-openapi
const mockUse = vi.fn()
const mockGet = vi.fn()
const mockHonoApp = {
  use: mockUse,
  get: mockGet,
  fetch: vi.fn(),
}
vi.mock('@hono/zod-openapi', () => ({
  OpenAPIHono: vi.fn().mockImplementation(() => mockHonoApp),
}))

// Mock @hono/node-server/serve-static
vi.mock('@hono/node-server/serve-static', () => ({
  serveStatic: vi.fn().mockReturnValue(() => {}),
}))

// Mock @hono/node-server
const mockServe = vi.fn()
vi.mock('@hono/node-server', () => ({
  serve: (...args: unknown[]) => mockServe(...args),
}))

import { registerPlaygroundCommand } from '../../src/commands/playground.js'

describe('registerPlaygroundCommand', () => {
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
  })

  it('starts playground server when dist exists', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerPlaygroundCommand(program)

    await program.parseAsync(['node', 'test', 'playground'])

    expect(mockServe).toHaveBeenCalled()
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Playground UI'))
  })

  it('exits with 1 when dist directory does not exist', async () => {
    mockExistsSync.mockReturnValue(false)

    const program = new Command()
    program.exitOverride()
    registerPlaygroundCommand(program)

    await program.parseAsync(['node', 'test', 'playground'])

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('Playground dist not found'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exits with 1 when playground assets cannot be read', async () => {
    mockExistsSync.mockImplementationOnce(() => { throw new Error('Cannot read assets') })

    const program = new Command()
    program.exitOverride()
    registerPlaygroundCommand(program)

    await program.parseAsync(['node', 'test', 'playground'])

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('Playground UI unavailable'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('uses custom port', async () => {
    mockExistsSync.mockReturnValue(true)
    mockServe.mockImplementation((_opts: unknown, cb: () => void) => {
      cb()
    })

    const program = new Command()
    program.exitOverride()
    registerPlaygroundCommand(program)

    await program.parseAsync(['node', 'test', 'playground', '-p', '8080'])

    const serveCall = mockServe.mock.calls[0]
    expect(serveCall[0].port).toBe(8080)
  })
})
