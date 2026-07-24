import {
  afterAll,
  beforeAll,
  describe,
  expect,
  it,
  vi,
} from 'vitest'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { createManagedRedisCommandClient } from '../../redis/src/managed.js'
import {
  redisAnyCommandMatcher,
  TcpFaultProxy,
} from '../../redis/tests/helpers/tcp-fault-proxy.js'
import { DependencyHealthRegistry } from '../src/dependency-health.js'

let container: StartedTestContainer
let proxy: TcpFaultProxy
let redisUrl: string

async function eventually(
  operation: () => void | Promise<void>,
  timeoutMs = 1_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      await operation()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 10))
    }
  }
  throw lastError
}

async function withDeadline<T>(
  operation: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs)
      }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

describe('managed Redis registry readiness', () => {
  beforeAll(async () => {
    container = await new GenericContainer('redis:7-alpine')
      .withExposedPorts(6379)
      .start()
    proxy = new TcpFaultProxy('127.0.0.1', container.getMappedPort(6379))
    await proxy.open()
    redisUrl = `redis://127.0.0.1:${proxy.port}`
  }, 60_000)

  afterAll(async () => {
    try {
      await proxy?.stop()
    } finally {
      await container?.stop()
    }
  })

  it('bounds one blackholed PING and releases every timed-out registry caller', async () => {
    await proxy.open()
    const health = new DependencyHealthRegistry({ logger: () => {} })
    const managed = await createManagedRedisCommandClient(redisUrl, {
      observer: health,
      readinessTimeoutMs: 250,
      random: () => 0,
    })
    const disconnect = vi.spyOn(managed.client, 'disconnect')
    const commandQueue = () =>
      (managed.client as unknown as {
        commandQueue: { length: number }
      }).commandQueue.length
    const queuedBefore = commandQueue()
    let activeCallers = 0
    let settledCallers = 0
    health.register('redisCommand', async () => {
      activeCallers += 1
      try {
        await managed.check()
      } finally {
        activeCallers -= 1
        settledCallers += 1
      }
    })

    try {
      await proxy.blackhole()
      const probes = Array.from(
        { length: 8 },
        () => health.checkReadiness(25),
      )

      await eventually(() => {
        expect(activeCallers).toBe(8)
        expect(commandQueue() - queuedBefore).toBe(1)
      })
      const results = await withDeadline(
        Promise.all(probes),
        500,
        'registry readiness responses did not meet their deadline',
      )
      expect(
        results.every((result) =>
          result.dependencies.redisCommand?.state === 'unhealthy'
          && result.dependencies.redisCommand.errorKind === 'timeout',
        ),
      ).toBe(true)
      expect(activeCallers).toBe(8)

      await eventually(() => {
        expect(activeCallers).toBe(0)
        expect(settledCallers).toBe(8)
        expect(commandQueue()).toBe(0)
      })
      expect(disconnect).toHaveBeenCalledWith(true)

      await proxy.open()
      await eventually(() => managed.check())
    } finally {
      disconnect.mockRestore()
      await managed.close()
      await proxy.open()
    }
  }, 20_000)

  it('reports a PING that succeeds after a generation transition as unhealthy', async () => {
    await proxy.open()
    const health = new DependencyHealthRegistry({ logger: () => {} })
    const managed = await createManagedRedisCommandClient(redisUrl, {
      observer: health,
      readinessTimeoutMs: 1_000,
      random: () => 0,
    })
    health.register('redisCommand', managed.check)
    const matchedBefore = proxy.matchedCommands

    try {
      proxy.holdNextResponse(redisAnyCommandMatcher('PING'))
      const readiness = health.checkReadiness(1_000)
      await eventually(() => {
        expect(proxy.matchedCommands - matchedBefore).toBe(1)
      })

      managed.client.emit('reconnecting', 25)
      proxy.releaseHeldResponse()

      await expect(readiness).resolves.toEqual({
        ok: false,
        dependencies: {
          redisCommand: {
            state: 'unhealthy',
            errorKind: 'connection_closed',
          },
        },
      })
    } finally {
      await managed.close()
      await proxy.open()
    }
  }, 20_000)
})
