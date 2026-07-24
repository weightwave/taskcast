import { describe, expect, it, vi } from 'vitest'
import {
  DependencyUnavailableError,
  type DependencyObservation,
  type Task,
  type TaskEvent,
} from '@taskcast/core'
import type { Redis } from 'ioredis'
import { RedisBroadcastProvider } from '../src/broadcast.js'
import { RedisShortTermStore } from '../src/short-term.js'

const task: Task = {
  id: 'task-1',
  status: 'pending',
  createdAt: 1,
  updatedAt: 1,
}

const event: TaskEvent = {
  id: 'event-1',
  taskId: task.id,
  index: 0,
  timestamp: 1,
  type: 'test.event',
  level: 'info',
  data: null,
}

function resetError(): Error {
  return Object.assign(new Error('socket reset'), { code: 'ECONNRESET' })
}

describe('managed Redis business operations', () => {
  it('wraps a store connection error, observes it, and preserves the original cause', async () => {
    const error = resetError()
    const observations: DependencyObservation[] = []
    const redis = {
      set: vi.fn().mockRejectedValue(error),
    } as unknown as Redis
    const store = new RedisShortTermStore(redis, {
      managed: true,
      observer: { observe: (observation) => observations.push(observation) },
    })

    await expect(store.saveTask(task)).rejects.toMatchObject({
      dependency: 'redisCommand',
      kind: 'connection_reset',
      cause: error,
    } satisfies Partial<DependencyUnavailableError>)
    expect(observations).toContainEqual({
      dependency: 'redisCommand',
      state: 'reconnecting',
      errorKind: 'connection_reset',
    })
    expect(redis.set).toHaveBeenCalledOnce()
  })

  it('wraps a pipeline per-command connection error without changing ordinary pipeline errors', async () => {
    const error = resetError()
    const observations: DependencyObservation[] = []
    const pipeline = {
      get: vi.fn(),
      exec: vi.fn().mockResolvedValue([[error, null]]),
    }
    const redis = {
      smembers: vi.fn().mockResolvedValue(['task-1']),
      pipeline: vi.fn().mockReturnValue(pipeline),
    } as unknown as Redis
    const store = new RedisShortTermStore(redis, {
      managed: true,
      observer: { observe: (observation) => observations.push(observation) },
    })

    await expect(store.listTasks({})).rejects.toMatchObject({
      dependency: 'redisCommand',
      kind: 'connection_reset',
      cause: error,
    } satisfies Partial<DependencyUnavailableError>)
    expect(observations).toContainEqual({
      dependency: 'redisCommand',
      state: 'reconnecting',
      errorKind: 'connection_reset',
    })
  })

  it('wraps publishing failures only for the managed command path', async () => {
    const error = resetError()
    const observations: DependencyObservation[] = []
    const pub = {
      publish: vi.fn().mockRejectedValue(error),
    } as unknown as Redis
    const sub = {
      on: vi.fn(),
    } as unknown as Redis
    const provider = new RedisBroadcastProvider(pub, sub, {
      managed: true,
      observer: { observe: (observation) => observations.push(observation) },
    })

    await expect(provider.publish('task-1', event)).rejects.toMatchObject({
      dependency: 'redisCommand',
      kind: 'connection_reset',
      cause: error,
    } satisfies Partial<DependencyUnavailableError>)
    expect(observations).toContainEqual({
      dependency: 'redisCommand',
      state: 'reconnecting',
      errorKind: 'connection_reset',
    })

    const raw = new RedisBroadcastProvider(pub, sub)
    await expect(raw.publish('task-1', event)).rejects.toBe(error)
  })
})
