import { describe, it, expect, afterEach } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { startTestServer } from '@taskcast/server/testing'
import { TaskcastServerClient } from '../../src/client.js'

async function startRealServer() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  return startTestServer({ engine, auth: { mode: 'none' } })
}

describe('Server-SDK integration — real HTTP server', () => {
  let close: (() => void) | undefined

  afterEach(() => close?.())

  it('createTask -> getTask returns consistent data', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const created = await sdk.createTask({ type: 'llm.chat' })
    expect(created.id).toBeTruthy()
    expect(created.status).toBe('pending')

    const fetched = await sdk.getTask(created.id)
    expect(fetched.id).toBe(created.id)
    expect(fetched.type).toBe('llm.chat')
  })

  it('full transition flow: pending -> running -> completed', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})

    const running = await sdk.transitionTask(task.id, 'running')
    expect(running.status).toBe('running')

    const completed = await sdk.transitionTask(task.id, 'completed', {
      result: { answer: 42 },
    })
    expect(completed.status).toBe('completed')
    expect(completed.result).toEqual({ answer: 42 })
  })

  it('publishEvent + getHistory returns events in order', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')

    await sdk.publishEvent(task.id, { type: 'e1', level: 'info', data: { n: 1 } })
    await sdk.publishEvent(task.id, { type: 'e2', level: 'info', data: { n: 2 } })
    await sdk.publishEvent(task.id, { type: 'e3', level: 'info', data: { n: 3 } })

    const history = await sdk.getHistory(task.id)
    const userEvents = history.filter(e => !e.type.startsWith('taskcast:'))
    expect(userEvents).toHaveLength(3)
    expect(userEvents.map(e => e.type)).toEqual(['e1', 'e2', 'e3'])
  })

  it('getTask for nonexistent task throws', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    await expect(sdk.getTask('nonexistent')).rejects.toThrow()
  })

  it('double complete throws conflict error', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')
    await sdk.transitionTask(task.id, 'completed')

    await expect(sdk.transitionTask(task.id, 'completed')).rejects.toThrow()
  })

  it('batch publish events', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')

    const inputs = Array.from({ length: 10 }, (_, i) => ({
      type: 'chunk', level: 'info' as const, data: { i },
    }))
    const results = await sdk.publishEvents(task.id, inputs)
    expect(results).toHaveLength(10)

    const history = await sdk.getHistory(task.id)
    const chunks = history.filter(e => e.type === 'chunk')
    expect(chunks).toHaveLength(10)
  })

  it('since pagination returns incremental results', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')

    await sdk.publishEvent(task.id, { type: 'e1', level: 'info', data: null })
    await sdk.publishEvent(task.id, { type: 'e2', level: 'info', data: null })
    await sdk.publishEvent(task.id, { type: 'e3', level: 'info', data: null })

    const all = await sdk.getHistory(task.id)
    // Get events after the second event (index-based)
    const partial = await sdk.getHistory(task.id, { since: { index: all[1]!.index } })
    expect(partial.length).toBeLessThan(all.length)
  })
})
