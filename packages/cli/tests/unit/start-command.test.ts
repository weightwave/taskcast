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
vi.mock('@taskcast/core', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@taskcast/core')>()
  return {
    ...actual,
    TaskEngine: vi.fn().mockImplementation(() => ({})),
    WorkerManager: vi.fn().mockImplementation(() => ({})),
    loadConfigFile: vi.fn().mockResolvedValue({ config: { port: 3721 }, source: 'none', path: undefined }),
    resolveAdminToken: vi.fn(),
    MemoryBroadcastProvider: vi.fn(),
    MemoryShortTermStore: vi.fn(),
  }
})

// Mock @taskcast/server — use inline object, no top-level refs
vi.mock('@taskcast/server', () => ({
  DependencyHealthRegistry: vi.fn().mockImplementation(() => ({
    register: vi.fn(),
  })),
  createTaskcastApp: vi.fn().mockReturnValue({
    app: { use: vi.fn(), get: vi.fn(), fetch: vi.fn() },
    stop: vi.fn(),
  }),
  parseLogLevel: vi.fn((value?: string) => {
    const normalized = value?.trim().toLowerCase() || 'info'
    if (['debug', 'info', 'warn', 'error'].includes(normalized)) return normalized
    throw new Error(
      `invalid TASKCAST_LOG_LEVEL "${value}"; expected debug, info, warn, or error`,
    )
  }),
}))

// Mock @taskcast/redis
vi.mock('@taskcast/redis', () => ({
  createManagedRedisAdapters: vi.fn().mockResolvedValue({
    broadcast: {},
    shortTermStore: {},
    commandCheck: vi.fn(),
    pubSubCheck: vi.fn(),
    close: vi.fn(),
  }),
}))

// Mock @taskcast/postgres
vi.mock('@taskcast/postgres', () => ({
  PostgresLongTermStore: vi.fn().mockImplementation(() => ({})),
  classifyPostgresConnectivity: vi.fn().mockReturnValue(undefined),
  postgresCheck: vi.fn().mockResolvedValue(undefined),
}))

// Mock postgres
vi.mock('postgres', () => ({
  default: vi.fn().mockImplementation(() => {
    const sql = vi.fn().mockResolvedValue([])
    return Object.assign(sql, { end: vi.fn().mockResolvedValue(undefined) })
  }),
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

import {
  parsePostgresMaxConnections,
  effectiveRuntimeAdapters,
  registerStartCommand,
  resolveStorageMode,
  runStart,
} from '../../src/commands/start.js'
import type { RunStartOptions } from '../../src/commands/start.js'

describe('storage and PostgreSQL option resolution', () => {
  it('reports the adapters actually selected after storage precedence', () => {
    expect(effectiveRuntimeAdapters('memory', false)).toEqual({
      broadcast: 'memory',
      shortTermStore: 'memory',
    })
    expect(effectiveRuntimeAdapters('redis', true)).toEqual({
      broadcast: 'redis',
      shortTermStore: 'redis',
      longTermStore: 'postgres',
    })
    expect(effectiveRuntimeAdapters('sqlite', true)).toEqual({
      broadcast: 'memory',
      shortTermStore: 'sqlite',
      longTermStore: 'sqlite',
    })
  })

  it.each([
    {
      name: 'explicit memory overrides configured Redis and a Redis URL',
      options: {
        cli: 'memory',
        configuredProvider: 'redis',
        hasRedisUrl: true,
      },
      expected: 'memory',
    },
    {
      name: 'explicit sqlite overrides env, config, and URL',
      options: {
        cli: 'sqlite',
        env: 'redis',
        configuredProvider: 'redis',
        hasRedisUrl: true,
      },
      expected: 'sqlite',
    },
    {
      name: 'env memory overrides configured Redis and a Redis URL',
      options: {
        env: 'memory',
        configuredProvider: 'redis',
        hasRedisUrl: true,
      },
      expected: 'memory',
    },
    {
      name: 'configured Redis is selected',
      options: {
        configuredProvider: 'redis',
        hasRedisUrl: true,
      },
      expected: 'redis',
    },
    {
      name: 'a Redis URL is auto-detected',
      options: { hasRedisUrl: true },
      expected: 'redis',
    },
    {
      name: 'memory is the final fallback',
      options: { hasRedisUrl: false },
      expected: 'memory',
    },
  ])('$name', ({ options, expected }) => {
    expect(resolveStorageMode(options)).toBe(expected)
  })

  it.each(['disk', '', 'REDIS'])('rejects invalid storage mode %j', (value) => {
    expect(() => resolveStorageMode({
      cli: value,
      hasRedisUrl: false,
    })).toThrow('invalid storage mode')
  })

  it.each([
    [undefined, 10],
    ['', 10],
    ['10', 10],
    ['1', 1],
  ])('parses PostgreSQL max connections %j as %i', (value, expected) => {
    expect(parsePostgresMaxConnections(value)).toBe(expected)
  })

  it.each(['0', '-1', '1.5', 'abc', '9007199254740992'])(
    'rejects invalid PostgreSQL max connections %j',
    (value) => {
      expect(() => parsePostgresMaxConnections(value)).toThrow(
        'TASKCAST_POSTGRES_MAX_CONNECTIONS must be a positive integer',
      )
    },
  )
})

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

  it('passes actual memory adapters to the server when a file config names Redis', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: {
        adapters: {
          broadcast: { provider: 'redis', url: 'redis://configured.example:6379' },
          shortTermStore: { provider: 'redis', url: 'redis://configured.example:6379' },
        },
      },
      source: 'file',
      path: '/fake/taskcast.config.yaml',
    })
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await program.parseAsync(['node', 'test', 'start', '--storage', 'memory'])

    const { createTaskcastApp } = await import('@taskcast/server')
    expect(createTaskcastApp).toHaveBeenLastCalledWith(expect.objectContaining({
      effectiveAdapters: {
        broadcast: 'memory',
        shortTermStore: 'memory',
      },
    }))
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721 },
      source: 'none',
      path: undefined,
    })
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

  it('prints postgres long-term store info when configured (display URL, credentials stripped)', async () => {
    const origPg = process.env['TASKCAST_POSTGRES_URL']
    const origRedis = process.env['TASKCAST_REDIS_URL']
    // Use a URL with credentials to verify they're stripped from the log.
    process.env['TASKCAST_POSTGRES_URL'] = 'postgres://user:secretpass@localhost:5432/taskcast'
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

    // The log line must contain host:port/db (display format), not the raw URL
    // with credentials.
    expect(logSpy).toHaveBeenCalledWith('[taskcast] Long-term store:  postgres')

    // Belt-and-suspenders: assert no log call includes the password
    const allCalls = logSpy.mock.calls.map((c) => String(c[0]))
    for (const call of allCalls) {
      expect(call).not.toContain('secretpass')
      expect(call).not.toContain('user:')
    }
  })

  it('prints redis short-term store info with credentials stripped from URL', async () => {
    const origPg = process.env['TASKCAST_POSTGRES_URL']
    const origRedis = process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_POSTGRES_URL']
    process.env['TASKCAST_REDIS_URL'] = 'redis://admin:supersecret@redis.example.com:6379/0'

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start'])
    } finally {
      if (origPg !== undefined) process.env['TASKCAST_POSTGRES_URL'] = origPg
      if (origRedis !== undefined) process.env['TASKCAST_REDIS_URL'] = origRedis
      else delete process.env['TASKCAST_REDIS_URL']
    }

    // The redis label must include the host but NOT the password.
    const allCalls = logSpy.mock.calls.map((c) => String(c[0]))
    for (const call of allCalls) {
      expect(call).not.toContain('supersecret')
      expect(call).not.toContain('admin:')
    }
    // Confirm a redis line was actually emitted (otherwise assertions above
    // would pass vacuously).
    const redisLines = allCalls.filter((c) => c.includes('redis @'))
    expect(redisLines.length).toBeGreaterThan(0)
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
    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    expect(createManagedRedisAdapters).toHaveBeenCalledWith(
      'redis://localhost:6379',
      expect.objectContaining({ startupTimeoutMs: 15_000 }),
    )
  })

  it('explicit memory ignores an unrelated Redis URL', async () => {
    const origRedis = process.env['TASKCAST_REDIS_URL']
    process.env['TASKCAST_REDIS_URL'] = 'redis://unrelated.invalid:6379'
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start', '--storage', 'memory'])
    } finally {
      if (origRedis === undefined) delete process.env['TASKCAST_REDIS_URL']
      else process.env['TASKCAST_REDIS_URL'] = origRedis
    }

    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    expect(createManagedRedisAdapters).not.toHaveBeenCalled()
  })

  it('rejects mixed configured short-term and broadcast providers', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      config: {
        adapters: {
          shortTermStore: { provider: 'memory' },
          broadcast: { provider: 'redis', url: 'redis://localhost:6379' },
        },
      },
      source: 'file',
      path: '/fake/taskcast.config.yaml',
    })
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    await expect(
      program.parseAsync(['node', 'test', 'start']),
    ).rejects.toMatchObject({ code: 1 })
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining('configured short-term and broadcast providers must match'),
    )
    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    expect(createManagedRedisAdapters).not.toHaveBeenCalled()
  })

  it('rejects conflicting configured Redis URLs before connecting', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      config: {
        adapters: {
          broadcast: {
            provider: 'redis',
            url: 'redis://broadcast-user:broadcast-secret@broadcast.internal:6379',
          },
          shortTermStore: {
            provider: 'redis',
            url: 'redis://store-user:store-secret@store.internal:6380',
          },
        },
      },
      source: 'file',
      path: '/fake/taskcast.config.yaml',
    })
    const originalRedis = process.env['TASKCAST_REDIS_URL']
    const originalStorage = process.env['TASKCAST_STORAGE']
    delete process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_STORAGE']
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await expect(
        program.parseAsync(['node', 'test', 'start']),
      ).rejects.toMatchObject({ code: 1 })
    } finally {
      if (originalRedis === undefined) delete process.env['TASKCAST_REDIS_URL']
      else process.env['TASKCAST_REDIS_URL'] = originalRedis
      if (originalStorage === undefined) delete process.env['TASKCAST_STORAGE']
      else process.env['TASKCAST_STORAGE'] = originalStorage
    }

    expect(errorSpy).toHaveBeenCalledWith(
      '[taskcast] configured Redis broadcast and short-term URLs must match',
    )
    const stderr = errorSpy.mock.calls
      .map(([message]) => String(message))
      .join('\n')
    expect(stderr).not.toContain('broadcast.internal')
    expect(stderr).not.toContain('store.internal')
    expect(stderr).not.toContain('broadcast-secret')
    expect(stderr).not.toContain('store-secret')
    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    expect(createManagedRedisAdapters).not.toHaveBeenCalled()
  })

  it('ignores conflicting configured Redis URLs when memory is explicitly active', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      config: {
        adapters: {
          broadcast: {
            provider: 'redis',
            url: 'redis://broadcast.internal:6379',
          },
          shortTermStore: {
            provider: 'redis',
            url: 'redis://store.internal:6380',
          },
        },
      },
      source: 'file',
      path: '/fake/taskcast.config.yaml',
    })
    const originalRedis = process.env['TASKCAST_REDIS_URL']
    const originalStorage = process.env['TASKCAST_STORAGE']
    delete process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_STORAGE']
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync([
        'node',
        'test',
        'start',
        '--storage',
        'memory',
      ])
    } finally {
      if (originalRedis === undefined) delete process.env['TASKCAST_REDIS_URL']
      else process.env['TASKCAST_REDIS_URL'] = originalRedis
      if (originalStorage === undefined) delete process.env['TASKCAST_STORAGE']
      else process.env['TASKCAST_STORAGE'] = originalStorage
    }

    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    expect(createManagedRedisAdapters).not.toHaveBeenCalled()
  })

  it.each([
    {
      name: 'equal',
      broadcastUrl: 'redis://shared.internal:6379',
      shortTermUrl: 'redis://shared.internal:6379',
      expected: 'redis://shared.internal:6379',
    },
    {
      name: 'single',
      broadcastUrl: 'redis://single.internal:6379',
      shortTermUrl: undefined,
      expected: 'redis://single.internal:6379',
    },
  ])('accepts $name configured Redis URL selection', async ({
    broadcastUrl,
    shortTermUrl,
    expected,
  }) => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      config: {
        adapters: {
          broadcast: { provider: 'redis', url: broadcastUrl },
          shortTermStore: {
            provider: 'redis',
            ...(shortTermUrl === undefined ? {} : { url: shortTermUrl }),
          },
        },
      },
      source: 'file',
      path: '/fake/taskcast.config.yaml',
    })
    const originalRedis = process.env['TASKCAST_REDIS_URL']
    const originalStorage = process.env['TASKCAST_STORAGE']
    delete process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_STORAGE']
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start'])
    } finally {
      if (originalRedis === undefined) delete process.env['TASKCAST_REDIS_URL']
      else process.env['TASKCAST_REDIS_URL'] = originalRedis
      if (originalStorage === undefined) delete process.env['TASKCAST_STORAGE']
      else process.env['TASKCAST_STORAGE'] = originalStorage
    }

    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    expect(createManagedRedisAdapters).toHaveBeenCalledWith(
      expected,
      expect.any(Object),
    )
  })

  it('rejects an explicit PostgreSQL provider without a resolved URL', async () => {
    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      config: {
        adapters: {
          longTermStore: { provider: 'postgres' },
        },
      },
      source: 'file',
      path: '/fake/taskcast.config.yaml',
    })
    const original = process.env['TASKCAST_POSTGRES_URL']
    delete process.env['TASKCAST_POSTGRES_URL']
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await expect(
        program.parseAsync(['node', 'test', 'start']),
      ).rejects.toMatchObject({ code: 1 })
    } finally {
      if (original === undefined) delete process.env['TASKCAST_POSTGRES_URL']
      else process.env['TASKCAST_POSTGRES_URL'] = original
    }

    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining(
        'configured PostgreSQL long-term store requires TASKCAST_POSTGRES_URL',
      ),
    )
    const postgresModule = await import('postgres')
    expect(postgresModule.default).not.toHaveBeenCalled()
  })

  it('does not bind when active Redis startup fails', async () => {
    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    ;(createManagedRedisAdapters as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error('Redis unavailable'),
    )
    const origRedis = process.env['TASKCAST_REDIS_URL']
    process.env['TASKCAST_REDIS_URL'] = 'redis://127.0.0.1:1'
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await expect(
        program.parseAsync(['node', 'test', 'start', '--storage', 'redis']),
      ).rejects.toMatchObject({ code: 1 })
    } finally {
      if (origRedis === undefined) delete process.env['TASKCAST_REDIS_URL']
      else process.env['TASKCAST_REDIS_URL'] = origRedis
    }

    const { serve } = await import('@hono/node-server')
    expect(serve).not.toHaveBeenCalled()
  })

  it('closes managed dependencies when HTTP listener binding fails', async () => {
    const redisClose = vi.fn().mockResolvedValue(undefined)
    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    ;(createManagedRedisAdapters as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      broadcast: {},
      shortTermStore: {},
      commandCheck: vi.fn(),
      pubSubCheck: vi.fn(),
      close: redisClose,
    })
    const { serve } = await import('@hono/node-server')
    ;(serve as ReturnType<typeof vi.fn>).mockImplementationOnce(() => ({
      close: vi.fn(),
      once: vi.fn((
        event: string,
        listener: (error: Error) => void,
      ) => {
        if (event === 'error') {
          queueMicrotask(() => listener(new Error('EADDRINUSE')))
        }
      }),
    }))
    const original = process.env['TASKCAST_REDIS_URL']
    process.env['TASKCAST_REDIS_URL'] = 'redis://127.0.0.1:6379'
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await expect(
        program.parseAsync([
          'node',
          'test',
          'start',
          '--storage',
          'redis',
        ]),
      ).rejects.toMatchObject({ code: 1 })
    } finally {
      if (original === undefined) delete process.env['TASKCAST_REDIS_URL']
      else process.env['TASKCAST_REDIS_URL'] = original
    }

    expect(redisClose).toHaveBeenCalledTimes(1)
  })

  it('does not bind and closes PostgreSQL when its startup check fails', async () => {
    const sql = Object.assign(vi.fn(), {
      end: vi.fn().mockResolvedValue(undefined),
    })
    const postgresModule = await import('postgres')
    ;(postgresModule.default as ReturnType<typeof vi.fn>).mockReturnValueOnce(sql)
    const { postgresCheck } = await import('@taskcast/postgres')
    ;(postgresCheck as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error(
        'startup failed at postgres://startup-user:startup-secret@'
        + 'private-startup.internal:6432/secret_database',
      ),
    )
    const origPg = process.env['TASKCAST_POSTGRES_URL']
    process.env['TASKCAST_POSTGRES_URL'] = 'postgres://127.0.0.1:1/taskcast'
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await expect(
        program.parseAsync(['node', 'test', 'start']),
      ).rejects.toMatchObject({ code: 1 })
    } finally {
      if (origPg === undefined) delete process.env['TASKCAST_POSTGRES_URL']
      else process.env['TASKCAST_POSTGRES_URL'] = origPg
    }

    const { serve } = await import('@hono/node-server')
    expect(serve).not.toHaveBeenCalled()
    expect(sql.end).toHaveBeenCalledTimes(1)
    expect(sql.end).toHaveBeenCalledWith({ timeout: 5 })
    expect(errorSpy).toHaveBeenCalledWith(
      '[taskcast] postgres unavailable (unavailable)',
    )
    const stderr = errorSpy.mock.calls
      .map(([message]) => String(message))
      .join('\n')
    expect(stderr).not.toContain('startup-secret')
    expect(stderr).not.toContain('private-startup.internal')
    expect(stderr).not.toContain('secret_database')
  })

  it('times out a blackholed PostgreSQL startup check before bind and closes once', async () => {
    vi.useFakeTimers()
    const validation = deferred<void>()
    const sql = Object.assign(vi.fn(), {
      end: vi.fn().mockResolvedValue(undefined),
    })
    const postgresModule = await import('postgres')
    ;(postgresModule.default as ReturnType<typeof vi.fn>).mockReturnValueOnce(sql)
    const { postgresCheck } = await import('@taskcast/postgres')
    ;(postgresCheck as ReturnType<typeof vi.fn>).mockReturnValueOnce(
      validation.promise,
    )
    const original = process.env['TASKCAST_POSTGRES_URL']
    process.env['TASKCAST_POSTGRES_URL'] =
      'postgres://startup-user:startup-secret@private-db.internal:5544/taskcast'
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)
    let settled = false
    const startup = program
      .parseAsync(['node', 'test', 'start'])
      .then(
        () => ({ status: 'fulfilled' as const }),
        (reason: unknown) => ({ status: 'rejected' as const, reason }),
      )
    void startup.then(() => {
      settled = true
    })

    try {
      await vi.waitFor(() => {
        expect(postgresCheck).toHaveBeenCalledTimes(1)
      })
      const { serve } = await import('@hono/node-server')
      expect(serve).not.toHaveBeenCalled()

      await vi.advanceTimersByTimeAsync(5_000)
      expect(settled).toBe(true)

      const outcome = await startup
      expect(outcome).toMatchObject({
        status: 'rejected',
        reason: { code: 1 },
      })
      expect(serve).not.toHaveBeenCalled()
      expect(sql.end).toHaveBeenCalledTimes(1)
      expect(sql.end).toHaveBeenCalledWith({ timeout: 5 })
      expect(errorSpy).toHaveBeenCalledWith(
        '[taskcast] postgres unavailable (timeout)',
      )
      const stderr = errorSpy.mock.calls
        .map(([message]) => String(message))
        .join('\n')
      expect(stderr).not.toContain('startup-secret')
      expect(stderr).not.toContain('private-db.internal')
      expect(stderr).not.toContain('5544')
    } finally {
      validation.reject(new Error('release test validation'))
      await startup
      if (original === undefined) delete process.env['TASKCAST_POSTGRES_URL']
      else process.env['TASKCAST_POSTGRES_URL'] = original
      vi.useRealTimers()
    }
  })

  it('sanitizes secret-bearing PostgreSQL migration failures', async () => {
    const sql = Object.assign(vi.fn(), {
      end: vi.fn().mockResolvedValue(undefined),
    })
    const postgresModule = await import('postgres')
    ;(postgresModule.default as ReturnType<typeof vi.fn>).mockReturnValueOnce(sql)
    const { performAutoMigrateIfEnabled } = await import(
      '../../src/auto-migrate.js'
    )
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>)
      .mockRejectedValueOnce(new Error(
        'migration failed at postgres://migration-user:migration-secret@'
        + 'private-migration.internal:6432/secret_database',
      ))
    const original = process.env['TASKCAST_POSTGRES_URL']
    process.env['TASKCAST_POSTGRES_URL'] =
      'postgres://configured-user:configured-secret@configured-db.internal:5432/configured_database'
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await expect(
        program.parseAsync(['node', 'test', 'start']),
      ).rejects.toMatchObject({ code: 1 })
    } finally {
      if (original === undefined) delete process.env['TASKCAST_POSTGRES_URL']
      else process.env['TASKCAST_POSTGRES_URL'] = original
    }

    expect(errorSpy).toHaveBeenCalledWith(
      '[taskcast] postgres unavailable (unavailable)',
    )
    const stderr = errorSpy.mock.calls
      .map(([message]) => String(message))
      .join('\n')
    for (const sensitive of [
      'migration-secret',
      'private-migration.internal',
      '6432',
      'secret_database',
      'configured-secret',
      'configured-db.internal',
    ]) {
      expect(stderr).not.toContain(sensitive)
    }
    const stdout = logSpy.mock.calls
      .map(([message]) => String(message))
      .join('\n')
    for (const sensitive of [
      'configured-db.internal',
      '5432',
      'configured_database',
    ]) {
      expect(stdout).not.toContain(sensitive)
    }
    expect(sql.end).toHaveBeenCalledTimes(1)
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

  it('logs [taskcast] <msg> exactly once and exits 1 when runStart throws', async () => {
    // Regression test for R2-I1: the .action() wrapper must produce exactly
    // one "[taskcast] Auto-migration failed: ..." line when runStart throws
    // (via performAutoMigrateIfEnabled), not a duplicate from the helper itself.
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error('Auto-migration failed: Checksum mismatch detected'),
    )

    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'start', '-s', 'sqlite'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    // Count calls that contain the auto-migration failure message
    const failureCalls = errorSpy.mock.calls.filter((call) =>
      String(call[0]).includes('Auto-migration failed'),
    )
    expect(failureCalls).toHaveLength(1)
    expect(failureCalls[0]?.[0]).toBe(
      '[taskcast] Auto-migration failed: Checksum mismatch detected',
    )
    expect(exitSpy).toHaveBeenCalledWith(1)

    // Reset the mock for subsequent tests
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>).mockReset()
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

  it('blocks server startup with a sanitized dependency error if auto-migrate fails', async () => {
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
    const migrationError = new Error(
      'Auto-migration failed: Checksum mismatch',
    )
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      migrationError,
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

    const startup = runStart(options)
    await expect(startup).rejects.toMatchObject({
      name: 'DependencyUnavailableError',
      dependency: 'postgres',
      kind: 'unavailable',
      message: 'postgres unavailable (unavailable)',
    })
    await startup.catch((error: unknown) => {
      expect((error as Error & { cause?: unknown }).cause).toBe(migrationError)
    })

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

  it('passes TASKCAST_LOG_LEVEL to createTaskcastApp', async () => {
    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
      env: { TASKCAST_LOG_LEVEL: 'ERROR' },
    }

    await runStart(options)

    const { createTaskcastApp } = await import('@taskcast/server')
    expect(createTaskcastApp).toHaveBeenCalledWith(
      expect.objectContaining({ logLevel: 'error' }),
    )
  })

  it('defaults TASKCAST_LOG_LEVEL to info', async () => {
    await runStart({
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
      env: {},
    })

    const { createTaskcastApp } = await import('@taskcast/server')
    expect(createTaskcastApp).toHaveBeenCalledWith(
      expect.objectContaining({ logLevel: 'info' }),
    )
  })

  it('rejects an invalid TASKCAST_LOG_LEVEL before startup work', async () => {
    await expect(runStart({
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
      env: { TASKCAST_LOG_LEVEL: 'trace' },
    })).rejects.toThrow('invalid TASKCAST_LOG_LEVEL "trace"')

    const { serve } = await import('@hono/node-server')
    const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
    expect(serve).not.toHaveBeenCalled()
    expect(performAutoMigrateIfEnabled).not.toHaveBeenCalled()
  })

  it('passes jwt config and trusted services to createTaskcastApp', async () => {
    const options: RunStartOptions = {
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {
        auth: {
          mode: 'jwt',
          jwt: {
            algorithm: 'HS256',
            secret: 'test-secret-that-is-long-enough',
          },
        },
        trustedServices: [
          {
            name: 'backend',
            key: 'service-key-that-is-long-enough',
            taskIds: '*',
            scope: ['*'],
          },
        ],
      },
      verbose: false,
      playground: false,
    }

    await runStart(options)

    const { createTaskcastApp } = await import('@taskcast/server')
    expect(createTaskcastApp).toHaveBeenCalledWith(
      expect.objectContaining({
        auth: {
          mode: 'jwt',
          jwt: {
            algorithm: 'HS256',
            secret: 'test-secret-that-is-long-enough',
          },
          trustedServices: [
            {
              name: 'backend',
              key: 'service-key-that-is-long-enough',
              taskIds: '*',
              scope: ['*'],
            },
          ],
        },
      }),
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

interface Deferred<T> {
  promise: Promise<T>
  resolve(value: T): void
  reject(error: unknown): void
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T | PromiseLike<T>) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

type AsyncSignalHandler = () => Promise<void>

function addedSignalHandler(
  signal: 'SIGINT' | 'SIGTERM',
  before: Set<Function>,
): AsyncSignalHandler | undefined {
  return process.listeners(signal).find((listener) =>
    !before.has(listener)) as AsyncSignalHandler | undefined
}

describe('startup signal lifecycle', () => {
  let beforeInt: Set<Function>
  let beforeTerm: Set<Function>
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>
  let originalRedis: string | undefined
  let originalPostgres: string | undefined

  beforeEach(async () => {
    vi.clearAllMocks()
    beforeInt = new Set(process.listeners('SIGINT'))
    beforeTerm = new Set(process.listeners('SIGTERM'))
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    originalRedis = process.env['TASKCAST_REDIS_URL']
    originalPostgres = process.env['TASKCAST_POSTGRES_URL']
    delete process.env['TASKCAST_REDIS_URL']
    delete process.env['TASKCAST_POSTGRES_URL']

    const { loadConfigFile } = await import('@taskcast/core')
    ;(loadConfigFile as ReturnType<typeof vi.fn>).mockResolvedValue({
      config: { port: 3721 },
      source: 'file',
      path: '/fake/taskcast.config.yaml',
    })
  })

  afterEach(() => {
    for (const listener of process.listeners('SIGINT')) {
      if (!beforeInt.has(listener)) {
        process.off('SIGINT', listener as NodeJS.SignalsListener)
      }
    }
    for (const listener of process.listeners('SIGTERM')) {
      if (!beforeTerm.has(listener)) {
        process.off('SIGTERM', listener as NodeJS.SignalsListener)
      }
    }
    if (originalRedis === undefined) delete process.env['TASKCAST_REDIS_URL']
    else process.env['TASKCAST_REDIS_URL'] = originalRedis
    if (originalPostgres === undefined) delete process.env['TASKCAST_POSTGRES_URL']
    else process.env['TASKCAST_POSTGRES_URL'] = originalPostgres
    exitSpy.mockRestore()
    logSpy.mockRestore()
    errorSpy.mockRestore()
  })

  it('installs signal handlers before managed acquisition and cancels later startup work', async () => {
    const managedReady = deferred<{
      broadcast: object
      shortTermStore: object
      commandCheck: ReturnType<typeof vi.fn>
      pubSubCheck: ReturnType<typeof vi.fn>
      close: ReturnType<typeof vi.fn>
    }>()
    const redisClose = vi.fn().mockResolvedValue(undefined)
    let handlersInstalledAtAcquire = false
    const { createManagedRedisAdapters } = await import('@taskcast/redis')
    ;(createManagedRedisAdapters as ReturnType<typeof vi.fn>)
      .mockImplementationOnce(() => {
        handlersInstalledAtAcquire =
          process.listeners('SIGINT').length === beforeInt.size + 1
          && process.listeners('SIGTERM').length === beforeTerm.size + 1
        return managedReady.promise
      })

    process.env['TASKCAST_REDIS_URL'] = 'redis://127.0.0.1:6379'
    process.env['TASKCAST_POSTGRES_URL'] =
      'postgres://127.0.0.1:5432/taskcast'
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)
    const startup = program.parseAsync([
      'node',
      'test',
      'start',
      '--storage',
      'redis',
    ])

    await vi.waitFor(() => {
      expect(createManagedRedisAdapters).toHaveBeenCalledTimes(1)
    })
    const earlyHandler = addedSignalHandler('SIGINT', beforeInt)
    const shutdown = earlyHandler?.()
    managedReady.resolve({
      broadcast: {},
      shortTermStore: {},
      commandCheck: vi.fn(),
      pubSubCheck: vi.fn(),
      close: redisClose,
    })
    await startup

    const lateHandler = earlyHandler
      ?? addedSignalHandler('SIGINT', beforeInt)
    const finalShutdown = shutdown ?? lateHandler?.()
    await finalShutdown

    expect(handlersInstalledAtAcquire).toBe(true)
    expect(earlyHandler).toBeDefined()
    const postgresModule = await import('postgres')
    expect(postgresModule.default).not.toHaveBeenCalled()
    const { serve } = await import('@hono/node-server')
    expect(serve).not.toHaveBeenCalled()
    expect(redisClose).toHaveBeenCalledTimes(1)
    expect(exitSpy).not.toHaveBeenCalled()
    expect(process.listeners('SIGINT')).toHaveLength(beforeInt.size)
    expect(process.listeners('SIGTERM')).toHaveLength(beforeTerm.size)
  })

  it('bounds signal cleanup behind a blackholed PostgreSQL validation query', async () => {
    vi.useFakeTimers()
    const validation = deferred<void>()
    const sql = Object.assign(vi.fn(), {
      end: vi.fn().mockResolvedValue(undefined),
    })
    const postgresModule = await import('postgres')
    ;(postgresModule.default as ReturnType<typeof vi.fn>).mockReturnValueOnce(sql)
    const { postgresCheck } = await import('@taskcast/postgres')
    ;(postgresCheck as ReturnType<typeof vi.fn>).mockReturnValueOnce(
      validation.promise,
    )
    process.env['TASKCAST_POSTGRES_URL'] =
      'postgres://127.0.0.1:5432/taskcast'
    const { serve } = await import('@hono/node-server')
    const program = new Command()
    program.exitOverride()
    registerStartCommand(program)
    const startup = program.parseAsync(['node', 'test', 'start'])

    try {
      await vi.waitFor(() => {
        expect(postgresCheck).toHaveBeenCalledTimes(1)
      })
      const terminate = addedSignalHandler('SIGTERM', beforeTerm)
      expect(terminate).toBeDefined()
      const shutdown = terminate!()

      expect(serve).not.toHaveBeenCalled()
      expect(sql.end).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(5_000)
      await startup
      await shutdown

      expect(serve).not.toHaveBeenCalled()
      expect(sql.end).toHaveBeenCalledTimes(1)
      expect(sql.end).toHaveBeenCalledWith({ timeout: 5 })
      expect(exitSpy).not.toHaveBeenCalled()
      expect(process.listeners('SIGINT')).toHaveLength(beforeInt.size)
      expect(process.listeners('SIGTERM')).toHaveLength(beforeTerm.size)
    } finally {
      validation.reject(new Error('release test validation'))
      await startup.catch(() => {})
      vi.useRealTimers()
    }
  })

  it('cancels after an in-flight migration without binding HTTP', async () => {
    const migration = deferred<void>()
    const { performAutoMigrateIfEnabled } = await import(
      '../../src/auto-migrate.js'
    )
    ;(performAutoMigrateIfEnabled as ReturnType<typeof vi.fn>)
      .mockReturnValueOnce(migration.promise)
    const closeDependencies = vi.fn().mockResolvedValue(undefined)
    const { serve } = await import('@hono/node-server')

    const startup = runStart({
      postgres: {} as ReturnType<typeof import('postgres').default>,
      postgresUrl: 'postgres://127.0.0.1:5432/taskcast',
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
      closeDependencies,
    })

    await vi.waitFor(() => {
      expect(performAutoMigrateIfEnabled).toHaveBeenCalledTimes(1)
    })
    const earlyHandler = addedSignalHandler('SIGTERM', beforeTerm)
    const shutdown = earlyHandler?.()
    migration.resolve(undefined)
    await startup

    const lateHandler = earlyHandler
      ?? addedSignalHandler('SIGTERM', beforeTerm)
    const finalShutdown = shutdown ?? lateHandler?.()
    await finalShutdown

    expect(earlyHandler).toBeDefined()
    expect(serve).not.toHaveBeenCalled()
    expect(closeDependencies).toHaveBeenCalledTimes(1)
    expect(process.listeners('SIGINT')).toHaveLength(beforeInt.size)
    expect(process.listeners('SIGTERM')).toHaveLength(beforeTerm.size)
  })

  it('keeps both handlers installed until repeated signals finish one slow cleanup', async () => {
    const cleanup = deferred<void>()
    const closeDependencies = vi.fn(() => cleanup.promise)
    const serverClose = vi.fn(
      (callback?: (error?: Error) => void) => callback?.(),
    )
    const { serve } = await import('@hono/node-server')
    ;(serve as ReturnType<typeof vi.fn>).mockImplementationOnce(
      (_options: unknown, listening: () => void) => {
        listening()
        return { close: serverClose }
      },
    )

    await runStart({
      broadcast: {},
      shortTermStore: {},
      port: 3721,
      config: {},
      verbose: false,
      playground: false,
      closeDependencies,
    })

    const interrupt = addedSignalHandler('SIGINT', beforeInt)
    const terminate = addedSignalHandler('SIGTERM', beforeTerm)
    expect(interrupt).toBeDefined()
    expect(terminate).toBeDefined()
    const first = interrupt!()
    await vi.waitFor(() => {
      expect(closeDependencies).toHaveBeenCalledTimes(1)
    })
    const listenersDuringCleanup = {
      interrupt: process.listeners('SIGINT').length,
      terminate: process.listeners('SIGTERM').length,
    }
    const second = terminate!()
    cleanup.resolve(undefined)
    await Promise.all([
      first ?? Promise.resolve(),
      second ?? Promise.resolve(),
    ])

    expect(first).toBeInstanceOf(Promise)
    expect(second).toBe(first)
    expect(listenersDuringCleanup).toEqual({
      interrupt: beforeInt.size + 1,
      terminate: beforeTerm.size + 1,
    })
    expect(serverClose).toHaveBeenCalledTimes(1)
    expect(closeDependencies).toHaveBeenCalledTimes(1)
    expect(process.listeners('SIGINT')).toHaveLength(beforeInt.size)
    expect(process.listeners('SIGTERM')).toHaveLength(beforeTerm.size)
  })
})
