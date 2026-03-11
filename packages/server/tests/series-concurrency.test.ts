import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createSSERouter, createSubscriberCounts } from '../src/routes/sse.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const subscriberCounts = createSubscriberCounts()
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createSSERouter(engine, subscriberCounts))
  return { app, engine }
}

async function collectSSEEvents(
  res: Response,
  count: number,
  timeoutMs = 5000,
): Promise<Array<{ event: string; data: string }>> {
  const reader = res.body!.getReader()
  const decoder = new TextDecoder()
  const collected: Array<{ event: string; data: string }> = []
  let buffer = ''

  const deadline = Date.now() + timeoutMs
  while (collected.length < count && Date.now() < deadline) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    const blocks = buffer.split('\n\n')
    buffer = blocks.pop() ?? ''
    for (const block of blocks) {
      if (!block.trim()) continue
      const lines = block.split('\n')
      const eventLine = lines.find((l) => l.startsWith('event:'))
      const dataLine = lines.find((l) => l.startsWith('data:'))
      if (eventLine && dataLine) {
        collected.push({
          event: eventLine.replace('event:', '').trim(),
          data: dataLine.replace('data:', '').trim(),
        })
      }
    }
  }

  reader.cancel()
  return collected
}

function parsePayload(ev: { data: string }) {
  return JSON.parse(ev.data) as Record<string, unknown>
}

function userDataEvents(events: Array<{ event: string; data: string }>) {
  return events
    .filter((e) => e.event === 'taskcast.event')
    .map((e) => parsePayload(e))
    .filter((e) => e.type !== 'taskcast:status')
}

describe('Series format concurrency and E2E', () => {
  // ─── 1. Rapid Publishing — All Deltas Received in Order ─────────────────────
  it('receives all deltas in order under rapid publishing', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const CHUNK_COUNT = 50

    // Schedule rapid publication of 50 accumulate events after SSE connects
    setTimeout(async () => {
      for (let i = 0; i < CHUNK_COUNT; i++) {
        await engine.publishEvent(task.id, {
          type: 'llm.chunk', level: 'info',
          data: { delta: `chunk-${i}` }, seriesId: 's1', seriesMode: 'accumulate',
        })
      }
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    // History: status(running) = 1
    // Live: 50 chunks + status(completed) + done = 52
    // Total: 53
    const events = await collectSSEEvents(res, 53, 10000)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(CHUNK_COUNT)
    // Verify order and completeness
    for (let i = 0; i < CHUNK_COUNT; i++) {
      expect((chunks[i]!.data as Record<string, unknown>).delta).toBe(`chunk-${i}`)
    }
  }, 15000)

  // ─── 2. Mid-Stream Join — Snapshot Complete, No Gaps ────────────────────────
  it('mid-stream join snapshot is complete with no gaps or duplication', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 30 events BEFORE subscribing
    const letters = 'abcdefghijklmnopqrstuvwxyzABCD'
    for (let i = 0; i < 30; i++) {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: letters[i] }, seriesId: 's1', seriesMode: 'accumulate',
      })
    }

    // Schedule 10 more events + completion after SSE connects
    setTimeout(async () => {
      for (let i = 0; i < 10; i++) {
        await engine.publishEvent(task.id, {
          type: 'llm.chunk', level: 'info',
          data: { delta: String(i) }, seriesId: 's1', seriesMode: 'accumulate',
        })
      }
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    // History: status(running) + 30 accumulate [collapsed to 1 snapshot] = 2
    // Live: 10 deltas + status(completed) + done = 12
    // Total: 14
    const events = await collectSSEEvents(res, 14, 10000)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(11) // 1 snapshot + 10 live deltas

    // First event should be a snapshot with accumulated value of all 30
    const snapshot = chunks[0]!
    expect(snapshot.seriesSnapshot).toBe(true)
    const expectedSnapshotValue = letters // all 30 characters
    expect((snapshot.data as Record<string, unknown>).delta).toBe(expectedSnapshotValue)

    // Live deltas follow
    for (let i = 0; i < 10; i++) {
      const chunk = chunks[i + 1]!
      expect(chunk.seriesSnapshot).toBeUndefined()
      expect((chunk.data as Record<string, unknown>).delta).toBe(String(i))
    }

    // Verify: snapshot.delta + concat(live deltas) == full expected string
    const snapshotDelta = (snapshot.data as Record<string, unknown>).delta as string
    const liveDeltasConcat = chunks.slice(1).map(
      (c) => (c.data as Record<string, unknown>).delta as string,
    ).join('')
    expect(snapshotDelta + liveDeltasConcat).toBe(letters + '0123456789')
  }, 15000)

  // ─── 3. Mixed Delta + Accumulated Subscribers ──────────────────────────────
  it('mixed subscribers receive correct formats simultaneously', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Schedule 10 events + completion after SSE connections
    setTimeout(async () => {
      for (let i = 0; i < 10; i++) {
        await engine.publishEvent(task.id, {
          type: 'llm.chunk', level: 'info',
          data: { delta: `p${i}` }, seriesId: 's1', seriesMode: 'accumulate',
        })
      }
      await engine.transitionTask(task.id, 'completed')
    }, 100)

    // Open both subscribers simultaneously
    const deltaPromise = app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    const accPromise = app.request(
      `/tasks/${task.id}/events?seriesFormat=accumulated`,
    )

    const [deltaRes, accRes] = await Promise.all([deltaPromise, accPromise])

    // History: status(running) = 1
    // Live: 10 chunks + status(completed) + done = 12
    // Total: 13
    const deltaEvents = await collectSSEEvents(deltaRes, 13, 10000)
    const deltaChunks = userDataEvents(deltaEvents)

    const accEvents = await collectSSEEvents(accRes, 13, 10000)
    const accChunks = userDataEvents(accEvents)

    expect(deltaChunks).toHaveLength(10)
    expect(accChunks).toHaveLength(10)

    // Delta subscriber got original deltas
    for (let i = 0; i < 10; i++) {
      expect((deltaChunks[i]!.data as Record<string, unknown>).delta).toBe(`p${i}`)
    }

    // Accumulated subscriber got running totals
    let running = ''
    for (let i = 0; i < 10; i++) {
      running += `p${i}`
      expect((accChunks[i]!.data as Record<string, unknown>).delta).toBe(running)
    }

    // Final accumulated value == concat of all deltas
    const allDeltas = deltaChunks.map(
      (c) => (c.data as Record<string, unknown>).delta as string,
    ).join('')
    const lastAccumulated = (accChunks[9]!.data as Record<string, unknown>).delta as string
    expect(allDeltas).toBe(lastAccumulated)
  }, 15000)

  // ─── 4. Disconnect + Reconnect with since Cursor ───────────────────────────
  it('reconnect with since cursor resumes without gaps or duplication', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 10 accumulate events
    for (let i = 0; i < 10; i++) {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: `v${i}` }, seriesId: 's1', seriesMode: 'accumulate',
      })
    }
    await engine.transitionTask(task.id, 'completed')

    // With includeStatus=false:
    // All events (after filtering out status): chunk-v0(0), ..., chunk-v9(9)
    // since.index=4 skips filteredIndex <= 4, keeps v5(5), v6(6), v7(7), v8(8), v9(9)
    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta&includeStatus=false&since.index=4`,
    )
    // 5 events (v5..v9) + done = 6
    const events = await collectSSEEvents(res, 6)
    const chunks = userDataEvents(events)

    // Should receive individual delta events, NOT snapshots
    expect(chunks).toHaveLength(5)
    for (let i = 0; i < 5; i++) {
      expect(chunks[i]!.seriesSnapshot).toBeUndefined()
      expect((chunks[i]!.data as Record<string, unknown>).delta).toBe(`v${i + 5}`)
    }

    // Done event present
    const done = events.find((e) => e.event === 'taskcast.done')
    expect(done).toBeDefined()
  }, 15000)

  // ─── 5. Multiple Series Interleaved ────────────────────────────────────────
  it('multiple series accumulate independently under interleaved publishing', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 10 events alternating between series 'alpha' and 'beta'
    for (let i = 0; i < 5; i++) {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: `a${i}` }, seriesId: 'alpha', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: `b${i}` }, seriesId: 'beta', seriesMode: 'accumulate',
      })
    }
    await engine.transitionTask(task.id, 'completed')

    // Subscribe (late-join, terminal replay) with includeStatus=false
    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta&includeStatus=false`,
    )
    // snapshotAlpha + snapshotBeta + done = 3
    const events = await collectSSEEvents(res, 3)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(2)

    // alpha snapshot
    const alphaSnapshot = chunks.find((c) => c.seriesId === 'alpha')
    expect(alphaSnapshot).toBeDefined()
    expect(alphaSnapshot!.seriesSnapshot).toBe(true)
    expect((alphaSnapshot!.data as Record<string, unknown>).delta).toBe('a0a1a2a3a4')

    // beta snapshot
    const betaSnapshot = chunks.find((c) => c.seriesId === 'beta')
    expect(betaSnapshot).toBeDefined()
    expect(betaSnapshot!.seriesSnapshot).toBe(true)
    expect((betaSnapshot!.data as Record<string, unknown>).delta).toBe('b0b1b2b3b4')

    // Done event
    const done = events.find((e) => e.event === 'taskcast.done')
    expect(done).toBeDefined()
  }, 15000)

  // ─── 6. E2E Full Flow — Create, Publish, Subscribe, Verify ─────────────────
  it('e2e: full flow with delta and accumulated subscribers', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Schedule: publish 5 events, then complete
    setTimeout(async () => {
      for (let i = 0; i < 5; i++) {
        await engine.publishEvent(task.id, {
          type: 'llm.chunk', level: 'info',
          data: { delta: `w${i}` }, seriesId: 's1', seriesMode: 'accumulate',
        })
      }
      await engine.transitionTask(task.id, 'completed')
    }, 100)

    // Open delta subscriber and accumulated subscriber — starts collecting from history replay
    const deltaPromise = app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    const accPromise = app.request(
      `/tasks/${task.id}/events?seriesFormat=accumulated`,
    )

    const [deltaRes, accRes] = await Promise.all([deltaPromise, accPromise])

    // History: status(running) = 1
    // Live: 5 chunks + status(completed) + done = 7
    // Total: 8
    const deltaEvents = await collectSSEEvents(deltaRes, 8, 10000)
    const deltaChunks = userDataEvents(deltaEvents)

    const accEvents = await collectSSEEvents(accRes, 8, 10000)
    const accChunks = userDataEvents(accEvents)

    // Verify delta subscriber got 5 deltas with original values
    expect(deltaChunks).toHaveLength(5)
    for (let i = 0; i < 5; i++) {
      expect((deltaChunks[i]!.data as Record<string, unknown>).delta).toBe(`w${i}`)
    }

    // Verify accumulated subscriber got 5 accumulated values
    expect(accChunks).toHaveLength(5)
    let accumulated = ''
    for (let i = 0; i < 5; i++) {
      accumulated += `w${i}`
      expect((accChunks[i]!.data as Record<string, unknown>).delta).toBe(accumulated)
    }

    // Verify: concat of all delta values == last accumulated value
    const allDeltas = deltaChunks.map(
      (c) => (c.data as Record<string, unknown>).delta as string,
    ).join('')
    expect(allDeltas).toBe(accumulated)
  }, 15000)

  // ─── 7. E2E Mid-Stream Join ────────────────────────────────────────────────
  it('e2e: mid-stream join gets snapshot then deltas, final value matches', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 5 events before subscribing
    for (let i = 0; i < 5; i++) {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: `pre${i}` }, seriesId: 's1', seriesMode: 'accumulate',
      })
    }

    // Schedule 3 more events + completion after SSE connects
    setTimeout(async () => {
      for (let i = 0; i < 3; i++) {
        await engine.publishEvent(task.id, {
          type: 'llm.chunk', level: 'info',
          data: { delta: `post${i}` }, seriesId: 's1', seriesMode: 'accumulate',
        })
      }
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    // History: status(running) + 5 accumulate [collapsed to 1 snapshot] = 2
    // Live: 3 deltas + status(completed) + done = 5
    // Total: 7
    const events = await collectSSEEvents(res, 7, 10000)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(4) // 1 snapshot + 3 live deltas

    // Snapshot
    const snapshot = chunks[0]!
    expect(snapshot.seriesSnapshot).toBe(true)
    const snapshotDelta = (snapshot.data as Record<string, unknown>).delta as string
    expect(snapshotDelta).toBe('pre0pre1pre2pre3pre4')

    // Live deltas
    for (let i = 0; i < 3; i++) {
      expect(chunks[i + 1]!.seriesSnapshot).toBeUndefined()
      expect((chunks[i + 1]!.data as Record<string, unknown>).delta).toBe(`post${i}`)
    }

    // Verify: snapshot + live deltas concatenated == final accumulated value
    const liveDeltasConcat = chunks.slice(1).map(
      (c) => (c.data as Record<string, unknown>).delta as string,
    ).join('')
    expect(snapshotDelta + liveDeltasConcat).toBe('pre0pre1pre2pre3pre4post0post1post2')
  }, 15000)

  // ─── 8. E2E Completed Task — Single Snapshot Per Series ────────────────────
  it('e2e: completed task returns single snapshot per series', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish events to 2 series
    for (let i = 0; i < 4; i++) {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: `x${i}` }, seriesId: 'seriesX', seriesMode: 'accumulate',
      })
    }
    for (let i = 0; i < 3; i++) {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: `y${i}` }, seriesId: 'seriesY', seriesMode: 'accumulate',
      })
    }
    await engine.transitionTask(task.id, 'completed')

    // Subscribe after completion with includeStatus=false
    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta&includeStatus=false`,
    )
    // snapshotX + snapshotY + done = 3
    const events = await collectSSEEvents(res, 3)
    const chunks = userDataEvents(events)

    // Exactly 1 snapshot per series
    expect(chunks).toHaveLength(2)

    const xSnapshot = chunks.find((c) => c.seriesId === 'seriesX')
    expect(xSnapshot).toBeDefined()
    expect(xSnapshot!.seriesSnapshot).toBe(true)
    expect((xSnapshot!.data as Record<string, unknown>).delta).toBe('x0x1x2x3')

    const ySnapshot = chunks.find((c) => c.seriesId === 'seriesY')
    expect(ySnapshot).toBeDefined()
    expect(ySnapshot!.seriesSnapshot).toBe(true)
    expect((ySnapshot!.data as Record<string, unknown>).delta).toBe('y0y1y2')

    // Done event received
    const done = events.find((e) => e.event === 'taskcast.done')
    expect(done).toBeDefined()
    expect(parsePayload(done!).reason).toBe('completed')
  }, 15000)

  // ─── 9. Non-Series Events Unaffected by seriesFormat ───────────────────────
  it('non-series events unaffected by seriesFormat parameter', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Schedule mix of accumulate series events + plain events
    setTimeout(async () => {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: 'hello' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'tool.call', level: 'info',
        data: { tool: 'search', query: 'test' },
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: ' world' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'log.message', level: 'info',
        data: { message: 'processing complete' },
      })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=accumulated`,
    )
    // History: status(running) = 1
    // Live: 4 events + status(completed) + done = 6
    // Total: 7
    const events = await collectSSEEvents(res, 7, 10000)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(4)

    // Series event 1 - accumulated
    expect(chunks[0]!.type).toBe('llm.chunk')
    expect((chunks[0]!.data as Record<string, unknown>).delta).toBe('hello')

    // Plain event - data unchanged
    expect(chunks[1]!.type).toBe('tool.call')
    const toolData = chunks[1]!.data as Record<string, unknown>
    expect(toolData.tool).toBe('search')
    expect(toolData.query).toBe('test')

    // Series event 2 - accumulated (running total)
    expect(chunks[2]!.type).toBe('llm.chunk')
    expect((chunks[2]!.data as Record<string, unknown>).delta).toBe('hello world')

    // Plain event - data unchanged
    expect(chunks[3]!.type).toBe('log.message')
    const logData = chunks[3]!.data as Record<string, unknown>
    expect(logData.message).toBe('processing complete')
  }, 15000)
})
