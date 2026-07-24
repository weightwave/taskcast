import { afterEach, describe, expect, it, vi } from 'vitest'
import { Command } from 'commander'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { createServer, type Server as NetServer } from 'node:net'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { registerStartCommand } from '../../src/commands/start.js'

const trackedEnv = [
  'TASKCAST_STORAGE',
  'TASKCAST_REDIS_URL',
  'TASKCAST_POSTGRES_URL',
  'TASKCAST_POSTGRES_MAX_CONNECTIONS',
  'TASKCAST_AUTO_MIGRATE',
] as const

class ExitError extends Error {
  constructor(readonly code: number) {
    super(`process.exit(${code})`)
  }
}

const tempDirs: string[] = []
const containers: StartedTestContainer[] = []

afterEach(async () => {
  await Promise.all(containers.splice(0).map((container) => container.stop()))
  await Promise.all(tempDirs.splice(0).map(async (dir) => {
    try {
      await rm(dir, { recursive: true, force: true })
    } catch {
      // SQLite can retain a Windows file handle until process teardown.
    }
  }))
  vi.restoreAllMocks()
})

async function tempConfig(contents = '{}'): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'taskcast-dependency-startup-'))
  tempDirs.push(dir)
  const path = join(dir, 'taskcast.config.yaml')
  await writeFile(path, contents)
  return path
}

async function availablePort(): Promise<number> {
  const server = createServer()
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (address === null || typeof address === 'string') {
    throw new Error('expected TCP address')
  }
  await new Promise<void>((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve())
  })
  return address.port
}

async function connectionProbe(): Promise<{
  server: NetServer
  port: number
  connections: () => number
}> {
  let count = 0
  const server = createServer((socket) => {
    count += 1
    socket.destroy()
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (address === null || typeof address === 'string') {
    throw new Error('expected TCP address')
  }
  return { server, port: address.port, connections: () => count }
}

async function closeNetServer(server: NetServer): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve())
  })
}

async function waitForHealth(port: number): Promise<void> {
  const deadline = Date.now() + 5_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/health`)
      if (response.ok) return
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  throw lastError ?? new Error('server did not become healthy')
}

async function waitForPortClosed(port: number): Promise<void> {
  const deadline = Date.now() + 5_000
  while (Date.now() < deadline) {
    try {
      const socket = await import('node:net').then(({ connect }) =>
        connect({ host: '127.0.0.1', port }))
      await new Promise<void>((resolve) => {
        socket.once('connect', () => {
          socket.destroy()
          resolve()
        })
        socket.once('error', () => resolve())
      })
      if (socket.destroyed && socket.remoteAddress === undefined) return
    } catch {
      return
    }
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  throw new Error(`port ${port} remained open`)
}

async function startCommand(
  args: string[],
): Promise<{ shutdown(): Promise<void> }> {
  const beforeTerm = new Set(process.listeners('SIGTERM'))
  const beforeInt = new Set(process.listeners('SIGINT'))
  const program = new Command()
  program.exitOverride()
  registerStartCommand(program)
  await program.parseAsync(['node', 'test', 'start', ...args])

  const term = process.listeners('SIGTERM').find((listener) => !beforeTerm.has(listener))
  const interrupt = process.listeners('SIGINT').find((listener) => !beforeInt.has(listener))
  if (!term || !interrupt) throw new Error('start command did not register signal handlers')

  return {
    async shutdown() {
      term()
      const portIndex = args.findIndex((arg) => arg === '--port')
      if (portIndex !== -1) await waitForPortClosed(Number(args[portIndex + 1]))
      process.off('SIGTERM', term)
      process.off('SIGINT', interrupt)
    },
  }
}

function withIsolatedEnv(values: Partial<Record<(typeof trackedEnv)[number], string>>) {
  const originals = Object.fromEntries(
    trackedEnv.map((key) => [key, process.env[key]]),
  )
  for (const key of trackedEnv) delete process.env[key]
  Object.assign(process.env, values)
  return () => {
    for (const key of trackedEnv) {
      const original = originals[key]
      if (original === undefined) delete process.env[key]
      else process.env[key] = original
    }
  }
}

describe('dependency activation at CLI startup', () => {
  it('memory plus an unrelated Redis URL opens no Redis connection', async () => {
    const probe = await connectionProbe()
    const restoreEnv = withIsolatedEnv({
      TASKCAST_REDIS_URL: `redis://127.0.0.1:${probe.port}`,
    })
    const port = await availablePort()
    const config = await tempConfig()
    let handle: Awaited<ReturnType<typeof startCommand>> | undefined
    try {
      handle = await startCommand([
        '--config', config,
        '--storage', 'memory',
        '--port', String(port),
      ])
      await waitForHealth(port)
      await new Promise((resolve) => setTimeout(resolve, 100))
      expect(probe.connections()).toBe(0)
    } finally {
      await handle?.shutdown()
      restoreEnv()
      await closeNetServer(probe.server)
    }
  })

  it('SQLite plus a PostgreSQL URL opens no PostgreSQL connection', async () => {
    const probe = await connectionProbe()
    const restoreEnv = withIsolatedEnv({
      TASKCAST_POSTGRES_URL: `postgres://user:pass@127.0.0.1:${probe.port}/taskcast`,
    })
    const port = await availablePort()
    const config = await tempConfig()
    let handle: Awaited<ReturnType<typeof startCommand>> | undefined
    try {
      handle = await startCommand([
        '--config', config,
        '--storage', 'sqlite',
        '--db-path', ':memory:',
        '--port', String(port),
      ])
      await waitForHealth(port)
      await new Promise((resolve) => setTimeout(resolve, 100))
      expect(probe.connections()).toBe(0)
    } finally {
      await handle?.shutdown()
      restoreEnv()
      await closeNetServer(probe.server)
    }
  })

  it('an explicit non-PostgreSQL long-term provider ignores an env URL', async () => {
    const probe = await connectionProbe()
    const restoreEnv = withIsolatedEnv({
      TASKCAST_POSTGRES_URL: `postgres://user:pass@127.0.0.1:${probe.port}/taskcast`,
    })
    const port = await availablePort()
    const config = await tempConfig(`
adapters:
  longTermStore:
    provider: memory
`)
    let handle: Awaited<ReturnType<typeof startCommand>> | undefined
    try {
      handle = await startCommand([
        '--config', config,
        '--storage', 'memory',
        '--port', String(port),
      ])
      await waitForHealth(port)
      await new Promise((resolve) => setTimeout(resolve, 100))
      expect(probe.connections()).toBe(0)
    } finally {
      await handle?.shutdown()
      restoreEnv()
      await closeNetServer(probe.server)
    }
  })

  it('active unreachable Redis fails before HTTP bind', async () => {
    const dependencyPort = await availablePort()
    const httpPort = await availablePort()
    const restoreEnv = withIsolatedEnv({
      TASKCAST_REDIS_URL: `redis://127.0.0.1:${dependencyPort}`,
    })
    const config = await tempConfig()
    vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    try {
      const program = new Command()
      program.exitOverride()
      registerStartCommand(program)
      await expect(program.parseAsync([
        'node', 'test', 'start',
        '--config', config,
        '--storage', 'redis',
        '--port', String(httpPort),
      ])).rejects.toMatchObject({ code: 1 })
      const listener = createServer()
      await new Promise<void>((resolve) =>
        listener.listen(httpPort, '127.0.0.1', resolve))
      await closeNetServer(listener)
    } finally {
      restoreEnv()
    }
  }, 20_000)

  it('active unreachable PostgreSQL fails before HTTP bind', async () => {
    const dependencyPort = await availablePort()
    const httpPort = await availablePort()
    const restoreEnv = withIsolatedEnv({
      TASKCAST_POSTGRES_URL:
        `postgres://user:pass@127.0.0.1:${dependencyPort}/taskcast`,
    })
    const config = await tempConfig()
    vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    try {
      const program = new Command()
      program.exitOverride()
      registerStartCommand(program)
      await expect(program.parseAsync([
        'node', 'test', 'start',
        '--config', config,
        '--port', String(httpPort),
      ])).rejects.toMatchObject({ code: 1 })
      const listener = createServer()
      await new Promise<void>((resolve) =>
        listener.listen(httpPort, '127.0.0.1', resolve))
      await closeNetServer(listener)
    } finally {
      restoreEnv()
    }
  }, 10_000)

  it('config-file Redis and PostgreSQL providers activate without env URLs', async () => {
    const restoreEnv = withIsolatedEnv({})
    const redis = await new GenericContainer('redis:7-alpine')
      .withExposedPorts(6379)
      .start()
    containers.push(redis)
    const pg = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'taskcast',
      })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()
    containers.push(pg)
    const config = await tempConfig(`
adapters:
  broadcast:
    provider: redis
    url: redis://127.0.0.1:${redis.getMappedPort(6379)}
  shortTermStore:
    provider: redis
    url: redis://127.0.0.1:${redis.getMappedPort(6379)}
  longTermStore:
    provider: postgres
    url: postgres://test:test@127.0.0.1:${pg.getMappedPort(5432)}/taskcast
`)
    const port = await availablePort()
    let handle: Awaited<ReturnType<typeof startCommand>> | undefined
    try {
      handle = await startCommand([
        '--config', config,
        '--port', String(port),
      ])
      await waitForHealth(port)
      const response = await fetch(`http://127.0.0.1:${port}/health/ready`)
      expect(response.status).toBe(200)
      const body = await response.json() as {
        dependencies: Record<string, unknown>
      }
      expect(Object.keys(body.dependencies).sort()).toEqual([
        'postgres',
        'redisCommand',
        'redisPubSub',
      ])
    } finally {
      await handle?.shutdown()
      restoreEnv()
    }
  }, 120_000)
})
