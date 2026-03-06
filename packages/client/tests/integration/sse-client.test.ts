import { describe, it, expect, afterEach } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { startTestServer } from '@taskcast/server/testing'
import { TaskcastClient } from '../../src/client.js'
import type { SSEEnvelope } from '@taskcast/core'

async function startRealServer() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const { baseUrl, close } = await startTestServer({ engine, auth: { mode: 'none' } })
  return { engine, baseUrl, close }
}

describe('Client integration — real SSE endpoint', () => {
  let close: (() => void) | undefined

  afterEach(() => close?.())

  it('receives events and done via real SSE', async () => {
    const { engine, baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Schedule events
    setTimeout(async () => {
      await engine.publishEvent(task.id, { type: 'chunk', level: 'info', data: { text: 'hi' } })
      await engine.transitionTask(task.id, 'completed')
    }, 100)

    const events: SSEEnvelope[] = []
    let doneReason = ''

    const client = new TaskcastClient({ baseUrl })
    await client.subscribe(task.id, {
      onEvent: (env) => events.push(env),
      onDone: (reason) => { doneReason = reason },
    })

    expect(events.length).toBeGreaterThan(0)
    expect(doneReason).toBe('completed')
  }, 15000)

  it('filter types only returns matching events', async () => {
    const { engine, baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const events: SSEEnvelope[] = []
    const client = new TaskcastClient({ baseUrl })
    await client.subscribe(task.id, {
      filter: { types: ['llm.*'], includeStatus: false },
      onEvent: (env) => events.push(env),
      onDone: () => {},
    })

    const types = events.map(e => e.type)
    expect(types).toContain('llm.delta')
    expect(types).not.toContain('tool.call')
  }, 15000)
})
