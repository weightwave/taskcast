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
  loadConfigFile: vi.fn().mockResolvedValue({ config: { port: 3721 }, source: 'none', path: undefined }),
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

// Mock auto-migrate
vi.mock('../../src/auto-migrate.js', () => ({
  performAutoMigrateIfEnabled: vi.fn(),
}))

import { registerStartCommand, runStart } from '../../src/commands/start.js'
import type { RunStartOptions } from '../../src/commands/start.js'

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

  it('prints config path and storage info on startup', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721 },
      source: 'global',
      path: '/home/user/.taskcast/taskcast.config.yaml',
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

    expect(logSpy).toHaveBeenCalledWith('[taskcast] Config: /home/user/.taskcast/taskcast.config.yaml')
    expect(logSpy).toHaveBeenCalledWith('[taskcast] Short-term store: memory')
    expect(logSpy).toHaveBeenCalledWith('[taskcast] Long-term store:  (none)')

    // Reset
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721 },
      source: 'none',
      path: undefined,
    })
  })

  it('prints (none) for config path when no config file found', async () => {
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

    expect(logSpy).toHaveBeenCalledWith('[taskcast] Config: (none)')
  })

  it('prints postgres long-term store info when configured', async () => {
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

    expect(logSpy).toHaveBeenCalledWith('[taskcast] Long-term store:  postgres @ postgres://localhost/taskcast')
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

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Short-term store: sqlite'))
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

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Short-term store: memory'))
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
      path: '/fake/taskcast.config.yaml',
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
      path: undefined,
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
      .mockResolvedValueOnce({ config: { port: 3721 }, source: 'none', path: undefined })
      .mockResolvedValueOnce({ config: { port: 4000 }, source: 'file', path: '/fake/config.yaml' })

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
      path: undefined,
    })
  })

  it('starts with custom SQLite db path', async () => {
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '-s', 'sqlite', '--db-path', '/tmp/my.db'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('sqlite @ /tmp/my.db'))
  })
})

describe('runStart', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let onSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation((() => {
      throw new ExitError(0)
    }) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    onSpy = vi.spyOn(process, 'on').mockImplementation((() => process) as never)
    vi.clearAllMocks()
  })

  afterEach(() => {
    exitSpy.mockRestore()
    logSpy.mockRestore()
    onSpy.mockRestore()
  })

  it('calls performAutoMigrateIfEnabled with sql + postgresUrl + env when postgres is configured', async () => {
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
    const mockPostgres = {} as ReturnType<typeof import('postgres').default>

    const options: RunStartOptions = {
      postgres: mockPostgres,
      postgresUrl: 'postgres://localhost/taskcast',
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
      env: { TASKCAST_AUTO_MIGRATE: 'true' },
    }

    await runStart(options)

    expect(performAutoMigrateIfEnabled).toHaveBeenCalledWith(
      mockPostgres,
      'postgres://localhost/taskcast',
      expect.objectContaining({ TASKCAST_AUTO_MIGRATE: 'true' }),
    )
  })

  it('still calls performAutoMigrateIfEnabled with undefined sql when postgres is not configured', async () => {
    // The decision to skip auto-migrate now lives inside performAutoMigrateIfEnabled
    // (based on whether a sql connection is present), not in runStart. runStart
    // always invokes the helper so that the skip-message log happens at the
    // correct place when TASKCAST_AUTO_MIGRATE is set but no Postgres is configured.
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')

    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
    }

    await runStart(options)

    expect(performAutoMigrateIfEnabled).toHaveBeenCalledWith(undefined, undefined, undefined)
  })

  it('blocks server startup if auto-migrate fails', async () => {
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error('Auto-migration failed: Checksum mismatch'),
    )

    const mockPostgres = {} as ReturnType<typeof import('postgres').default>

    const options: RunStartOptions = {
      postgres: mockPostgres,
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
    }

    await expect(runStart(options)).rejects.toThrow('Auto-migration failed: Checksum mismatch')

    const { serve } = await import('@hono/node-server')
    expect(serve).not.toHaveBeenCalled()
  })

  it('starts server normally when auto-migrate succeeds', async () => {
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>).mockResolvedValueOnce(undefined)

    const mockPostgres = {} as ReturnType<typeof import('postgres').default>

    const options: RunStartOptions = {
      postgres: mockPostgres,
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
    }

    await runStart(options)

    const { serve } = await import('@hono/node-server')
    expect(serve).toHaveBeenCalled()
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Server started'))
  })

  it('starts server successfully without postgres', async () => {
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')

    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
    }

    await runStart(options)

    const { serve } = await import('@hono/node-server')
    expect(serve).toHaveBeenCalled()
    // performAutoMigrateIfEnabled is still called (to let it log the skip message
    // if TASKCAST_AUTO_MIGRATE is set), but with sql=undefined.
    expect(performAutoMigrateIfEnabled).toHaveBeenCalledWith(undefined, undefined, undefined)
  })

  it('passes verbose flag to createTaskcastApp', async () => {
    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: true,
      playground: false,
    }

    await runStart(options)

    const { createTaskcastApp } = await import('@taskcast/server')
    expect(createTaskcastApp).toHaveBeenCalledWith(
      expect.objectContaining({ verbose: true }),
    )
  })

  it('sets up long-term store when provided', async () => {
    const { TaskEngine } = await import('@taskcast/core')

    const mockLongTermStore = {}

    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      longTermStore: mockLongTermStore as any,
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
    }

    await runStart(options)

    // longTermStore is passed to TaskEngine, not createTaskcastApp
    expect(TaskEngine).toHaveBeenCalledWith(
      expect.objectContaining({
        longTermStore: mockLongTermStore,
      }),
    )
  })

  it('registers SIGTERM and SIGINT handlers', async () => {
    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
    }

    await runStart(options)

    expect(onSpy).toHaveBeenCalledWith('SIGTERM', expect.any(Function))
    expect(onSpy).toHaveBeenCalledWith('SIGINT', expect.any(Function))
  })

  it('uses correct port from options', async () => {
    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 4000,
      config: {},
      verbose: false,
      playground: false,
    }

    await runStart(options)

    const { serve } = await import('@hono/node-server')
    const serveCall = (serve as ReturnType<typeof vi.fn>).mock.calls[0]
    expect(serveCall[0].port).toBe(4000)
  })

  it('serves playground when playground flag is true and dist exists', async () => {
    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: true,
    }

    await runStart(options)

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Playground UI'))
  })

  it('creates engine with broadcast and shortTermStore', async () => {
    const { TaskEngine } = await import('@taskcast/core')

    const options: RunStartOptions = {
      broadcast: { mock: 'broadcast' } as any,
      shortTermStore: { mock: 'store' } as any,
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
    }

    await runStart(options)

    expect(TaskEngine).toHaveBeenCalledWith(
      expect.objectContaining({
        broadcast: { mock: 'broadcast' },
        shortTermStore: { mock: 'store' },
      }),
    )
  })

  it('auto-migrate receives correct env variables', async () => {
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>).mockResolvedValueOnce(undefined)

    const mockPostgres = {} as ReturnType<typeof import('postgres').default>
    const customEnv = { TASKCAST_AUTO_MIGRATE: 'true', CUSTOM_VAR: 'value' }

    const options: RunStartOptions = {
      postgres: mockPostgres,
      postgresUrl: 'postgres://custom/db',
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
      env: customEnv,
    }

    await runStart(options)

    expect(performAutoMigrateIfEnabled).toHaveBeenCalledWith(
      mockPostgres,
      'postgres://custom/db',
      customEnv,
    )
  })
})
