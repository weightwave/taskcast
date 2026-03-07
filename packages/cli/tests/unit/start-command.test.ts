import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

// Helper: make process.exit throw so execution stops (matching real behavior)
class ExitError extends Error {
  code: number
  constructor(code: number) {
    super(`process.exit(${code})`)
    this.code = code
  }
}

// Mock @taskcast/core
vi.mock('@taskcast/core', () => ({
  TaskEngine: vi.fn().mockImplementation(() => ({})),
  WorkerManager: vi.fn().mockImplementation(() => ({})),
  loadConfigFile: vi.fn().mockResolvedValue({ config: { port: 3721 }, source: 'none' }),
  resolveAdminToken: vi.fn(),
  MemoryBroadcastProvider: vi.fn(),
  MemoryShortTermStore: vi.fn(),
}))

// Mock @taskcast/server — use inline object, no top-level refs
vi.mock('@taskcast/server', () => ({
  createTaskcastApp: vi.fn().mockReturnValue({
    app: { use: vi.fn(), get: vi.fn(), fetch: vi.fn() },
    stop: vi.fn(),
  }),
}))

// Mock @taskcast/redis
vi.mock('@taskcast/redis', () => ({
  createRedisAdapters: vi.fn().mockReturnValue({
    broadcast: {},
    shortTermStore: {},
  }),
}))

// Mock ioredis
vi.mock('ioredis', () => ({
  Redis: vi.fn(),
}))

// Mock @taskcast/postgres
vi.mock('@taskcast/postgres', () => ({
  PostgresLongTermStore: vi.fn(),
}))

// Mock postgres
vi.mock('postgres', () => ({
  default: vi.fn(),
}))

// Mock @taskcast/sqlite
vi.mock('@taskcast/sqlite', () => ({
  createSqliteAdapters: vi.fn().mockReturnValue({
    shortTermStore: {},
    longTermStore: {},
  }),
}))

// Mock utils
vi.mock('../../src/utils.js', () => ({
  promptCreateGlobalConfig: vi.fn().mockResolvedValue(false),
  createDefaultGlobalConfig: vi.fn().mockReturnValue(null),
}))

// Mock @hono/node-server
vi.mock('@hono/node-server', () => ({
  serve: vi.fn().mockImplementation((_opts: unknown, cb: () => void) => {
    cb()
    return { close: vi.fn() }
  }),
}))

// Mock @hono/node-server/serve-static
vi.mock('@hono/node-server/serve-static', () => ({
  serveStatic: vi.fn().mockReturnValue(() => {}),
}))

// Mock module for createRequire
vi.mock('module', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    createRequire: () => ({ resolve: vi.fn().mockReturnValue('/fake/playground/package.json') }),
  }
})

// Mock fs for existsSync
vi.mock('fs', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    existsSync: vi.fn().mockReturnValue(true),
  }
})

import { registerStartCommand } from '../../src/commands/start.js'

describe('registerStartCommand', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let warnSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>
  let onSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    onSpy = vi.spyOn(process, 'on').mockImplementation((() => process) as never)
    vi.clearAllMocks()
  })

  afterEach(() => {
    exitSpy.mockRestore()
    logSpy.mockRestore()
    warnSpy.mockRestore()
    errorSpy.mockRestore()
    onSpy.mockRestore()
  })

  it('starts server with memory storage by default', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start'])

    const { serve } = await import('@hono/node-server')
    expect(serve).toHaveBeenCalled()
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Server started'))
  })

  it('starts server with custom port', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '-p', '4000'])

    const { serve } = await import('@hono/node-server')
    const serveCall = (serve as ReturnType<typeof vi.fn>).mock.calls[0]
    expect(serveCall[0].port).toBe(4000)
  })

  it('starts server with sqlite storage', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '-s', 'sqlite'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('SQLite'))
  })

  it('starts server with redis storage', async () => {
    const origEnv = process.env['TASKCAST_REDIS_URL']
    process.env['TASKCAST_REDIS_URL'] = 'redis://localhost:6379'

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start', '-s', 'redis'])
    } finally {
      if (origEnv !== undefined) {
        process.env['TASKCAST_REDIS_URL'] = origEnv
      } else {
        delete process.env['TASKCAST_REDIS_URL']
      }
    }

    const { serve } = await import('@hono/node-server')
    expect(serve).toHaveBeenCalled()
  })

  it('starts server with playground flag', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '--playground'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Playground UI'))
  })

  it('registers SIGTERM and SIGINT handlers', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start'])

    expect(onSpy).toHaveBeenCalledWith('SIGTERM', expect.any(Function))
    expect(onSpy).toHaveBeenCalledWith('SIGINT', expect.any(Function))
  })

  it('uses in-memory adapters and warns when no redis URL configured', async () => {
    const origEnv = process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_REDIS_URL']

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start'])
    } finally {
      if (origEnv !== undefined) process.env['TASKCAST_REDIS_URL'] = origEnv
    }

    expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('in-memory adapters'))
  })

  it('passes verbose flag to server', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '-v'])

    const { createTaskcastApp } = await import('@taskcast/server')
    expect(createTaskcastApp).toHaveBeenCalledWith(
      expect.objectContaining({ verbose: true }),
    )
  })

  it('sets up PostgreSQL long term store when TASKCAST_POSTGRES_URL is set', async () => {
    const origPg = process.env['TASKCAST_POSTGRES_URL']
    const origRedis = process.env['TASKCAST_REDIS_URL']
    process.env['TASKCAST_POSTGRES_URL'] = 'postgres://localhost/taskcast'
    delete process.env['TASKCAST_REDIS_URL']

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start'])
    } finally {
      if (origPg !== undefined) process.env['TASKCAST_POSTGRES_URL'] = origPg
      else delete process.env['TASKCAST_POSTGRES_URL']
      if (origRedis !== undefined) process.env['TASKCAST_REDIS_URL'] = origRedis
    }

    const { PostgresLongTermStore } = await import('@taskcast/postgres')
    expect(PostgresLongTermStore).toHaveBeenCalled()
  })

  it('sets up worker manager when config enables workers', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721, workers: { enabled: true, defaults: { maxRetries: 3 } } },
      source: 'file',
    })

    const origRedis = process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_REDIS_URL']

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start'])
    } finally {
      if (origRedis !== undefined) process.env['TASKCAST_REDIS_URL'] = origRedis
    }

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Worker assignment system enabled'))
    const { WorkerManager } = await import('@taskcast/core')
    expect(WorkerManager).toHaveBeenCalled()

    // Reset loadConfigFile
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721 },
      source: 'none',
    })
  })

  it('warns when playground dist not found', async () => {
    const { existsSync } = await import('fs')
    ;(existsSync as ReturnType<typeof vi.fn>).mockReturnValue(false)

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '--playground'])

    expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('Playground dist not found'))

    ;(existsSync as ReturnType<typeof vi.fn>).mockReturnValue(true)
  })

  it('warns when @taskcast/playground module not available', async () => {
    // Make createRequire throw
    const moduleImport = await import('module')
    const origCreateRequire = moduleImport.createRequire
    ;(moduleImport as any).createRequire = () => ({
      resolve: () => { throw new Error('MODULE_NOT_FOUND') },
    })

    const { existsSync } = await import('fs')
    ;(existsSync as ReturnType<typeof vi.fn>).mockReturnValue(true)

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '--playground'])

    // Should still warn — the catch block handles it
    expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('@taskcast/playground not available'))

    ;(moduleImport as any).createRequire = origCreateRequire
  })

  it('creates global config when source is none and user confirms', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721 },
      source: 'none',
    })

    const { promptCreateGlobalConfig, createDefaultGlobalConfig } = await import('../../src/utils.js')
    ;(promptCreateGlobalConfig as ReturnType<typeof vi.fn>).mockResolvedValue(true)
    ;(createDefaultGlobalConfig as ReturnType<typeof vi.fn>).mockReturnValue('/fake/config.yaml')

    // loadConfigFile will be called again with the created path
    ;(loadConfigFile as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce({ config: { port: 3721 }, source: 'none' })
      .mockResolvedValueOnce({ config: { port: 4000 }, source: 'file' })

    const origRedis = process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_REDIS_URL']

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start'])
    } finally {
      if (origRedis !== undefined) process.env['TASKCAST_REDIS_URL'] = origRedis
    }

    expect(promptCreateGlobalConfig).toHaveBeenCalled()
    expect(createDefaultGlobalConfig).toHaveBeenCalled()

    // Reset mocks
    ;(promptCreateGlobalConfig as ReturnType<typeof vi.fn>).mockResolvedValue(false)
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721 },
      source: 'none',
    })
  })

  it('starts with custom SQLite db path', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '-s', 'sqlite', '--db-path', '/tmp/my.db'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('/tmp/my.db'))
  })
})
