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

/** Extract only taskcast.event entries (not status, not done) */
function userDataEvents(events: Array<{ event: string; data: string }>) {
  return events
    .filter((e) => e.event === 'taskcast.event')
    .map((e) => parsePayload(e))
    .filter((e) => e.type !== 'taskcast:status')
}

describe('SSE seriesFormat', () => {
  // ─── 1. seriesFormat=delta from start (live) ────────────────────────────────
  // Subscribe to a running task, then publish accumulate events.
  // With seriesFormat=delta (default), each event should carry original delta data.
  it('seriesFormat=delta delivers original delta data for live accumulate events', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Schedule publishing after SSE connection is established.
    // includeStatus=true (default) so the terminal taskcast:status event
    // triggers stream close via taskcast.done.
    setTimeout(async () => {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: 'Hello' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: ' world' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: '!' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    // History: status(running)
    // Live: 3 chunks + status(completed) + done = 5
    // Total: 1 + 5 = 6
    const events = await collectSSEEvents(res, 6)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(3)
    // Each event has its original delta, not accumulated
    expect((chunks[0]!.data as Record<string, unknown>).delta).toBe('Hello')
    expect((chunks[1]!.data as Record<string, unknown>).delta).toBe(' world')
    expect((chunks[2]!.data as Record<string, unknown>).delta).toBe('!')
  }, 10000)

  // ─── 2. seriesFormat=accumulated from start (live) ──────────────────────────
  // Same setup: subscribe to a running task, publish accumulate events.
  // With seriesFormat=accumulated, each event should carry the running total.
  it('seriesFormat=accumulated delivers accumulated data for each live event', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    setTimeout(async () => {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: 'Hello' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: ' world' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: '!' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=accumulated`,
    )
    // History: status(running)
    // Live: 3 chunks + status(completed) + done
    // Total: 6
    const events = await collectSSEEvents(res, 6)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(3)
    // Each event has accumulated data
    expect((chunks[0]!.data as Record<string, unknown>).delta).toBe('Hello')
    expect((chunks[1]!.data as Record<string, unknown>).delta).toBe('Hello world')
    expect((chunks[2]!.data as Record<string, unknown>).delta).toBe('Hello world!')
  }, 10000)

  // ─── 3. Late-join with delta format ─────────────────────────────────────────
  // Task is running, 5 accumulate events already published.
  // Subscribe (no since cursor) — should get a single snapshot for replay,
  // then live deltas for subsequent events.
  it('late-join with seriesFormat=delta collapses history to snapshot then sends deltas', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 5 accumulate events BEFORE subscribing
    for (let i = 1; i <= 5; i++) {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: `chunk${i}` }, seriesId: 's1', seriesMode: 'accumulate',
      })
    }

    // Schedule 2 more events + completion after SSE connects
    setTimeout(async () => {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: '-live1' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: '-live2' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    // History: status(running) + 5 accumulate [collapsed to 1 snapshot] = 2
    // Live: 2 deltas + status(completed) + done = 4
    // Total: 6
    const events = await collectSSEEvents(res, 6)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(3) // 1 snapshot + 2 live deltas

    // First event should be a snapshot with accumulated value
    expect(chunks[0]!.seriesSnapshot).toBe(true)
    expect(chunks[0]!.seriesId).toBe('s1')
    const snapshotData = chunks[0]!.data as Record<string, unknown>
    expect(snapshotData.delta).toBe('chunk1chunk2chunk3chunk4chunk5')

    // Live events should be deltas (no snapshot flag)
    expect(chunks[1]!.seriesSnapshot).toBeUndefined()
    expect((chunks[1]!.data as Record<string, unknown>).delta).toBe('-live1')
    expect(chunks[2]!.seriesSnapshot).toBeUndefined()
    expect((chunks[2]!.data as Record<string, unknown>).delta).toBe('-live2')
  }, 10000)

  // ─── 4. Late-join with accumulated format ───────────────────────────────────
  // Same as above but seriesFormat=accumulated.
  // Snapshot replays accumulated, live events carry running totals.
  it('late-join with seriesFormat=accumulated sends snapshot then accumulated values', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 3 events before subscribing
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'A' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'B' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'C' }, seriesId: 's1', seriesMode: 'accumulate',
    })

    // Schedule live event + completion
    setTimeout(async () => {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: 'D' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=accumulated`,
    )
    // History: status(running) + snapshot = 2
    // Live: 1 chunk + status(completed) + done = 3
    // Total: 5
    const events = await collectSSEEvents(res, 5)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(2) // 1 snapshot + 1 live

    // Snapshot has accumulated value
    expect(chunks[0]!.seriesSnapshot).toBe(true)
    const snapshotData = chunks[0]!.data as Record<string, unknown>
    expect(snapshotData.delta).toBe('ABC')

    // Live event has accumulated data (ABCD not just D)
    const liveData = chunks[1]!.data as Record<string, unknown>
    expect(liveData.delta).toBe('ABCD')
  }, 10000)

  // ─── 5. Terminal task replay ────────────────────────────────────────────────
  // Task already completed. Subscribe to get replay.
  // Accumulate series are collapsed to snapshots, then done is sent.
  it('terminal task replays snapshot per series then sends done', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'part1' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'part2' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'part3' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.transitionTask(task.id, 'completed')

    // Subscribe after task is completed (includeStatus=false to isolate data events)
    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta&includeStatus=false`,
    )
    // snapshot + taskcast.done = 2
    const events = await collectSSEEvents(res, 2)

    const chunks = userDataEvents(events)
    expect(chunks).toHaveLength(1)
    expect(chunks[0]!.seriesSnapshot).toBe(true)
    expect((chunks[0]!.data as Record<string, unknown>).delta).toBe('part1part2part3')

    // Done event
    const done = events.find((e) => e.event === 'taskcast.done')
    expect(done).toBeDefined()
    expect(parsePayload(done!).reason).toBe('completed')
  })

  // ─── 6. Reconnect with since cursor does NOT collapse ──────────────────────
  // Providing a since cursor means the client is reconnecting, so no collapse.
  it('reconnect with since.index does not collapse accumulate events', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'a' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'b' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'c' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'd' }, seriesId: 's1', seriesMode: 'accumulate',
    })
    await engine.transitionTask(task.id, 'completed')

    // With includeStatus=false:
    // All events (after filtering out status): chunk-a(0), chunk-b(1), chunk-c(2), chunk-d(3)
    // since.index=1 skips filteredIndex<=1, keeps chunk-c(2) and chunk-d(3)
    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta&includeStatus=false&since.index=1`,
    )
    const events = await collectSSEEvents(res, 3) // chunk-c + chunk-d + done
    const chunks = userDataEvents(events)

    // Should receive individual delta events, NOT snapshots
    expect(chunks).toHaveLength(2)
    expect(chunks[0]!.seriesSnapshot).toBeUndefined()
    expect((chunks[0]!.data as Record<string, unknown>).delta).toBe('c')
    expect(chunks[1]!.seriesSnapshot).toBeUndefined()
    expect((chunks[1]!.data as Record<string, unknown>).delta).toBe('d')
  })

  // ─── 7. Multiple series ────────────────────────────────────────────────────
  // Two accumulate series + a non-series event.
  // Late-join should produce one snapshot per series, non-series events preserved.
  it('late-join produces one snapshot per series, preserves non-series events', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Series A
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'A1' }, seriesId: 'sA', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'A2' }, seriesId: 'sA', seriesMode: 'accumulate',
    })

    // Non-series event
    await engine.publishEvent(task.id, {
      type: 'tool.call', level: 'info',
      data: { tool: 'search', args: {} },
    })

    // Series B
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'B1' }, seriesId: 'sB', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.chunk', level: 'info',
      data: { delta: 'B2' }, seriesId: 'sB', seriesMode: 'accumulate',
    })

    await engine.transitionTask(task.id, 'completed')

    // Late-join after terminal (includeStatus=false to simplify assertions)
    const res = await app.request(
      `/tasks/${task.id}/events?seriesFormat=delta&includeStatus=false`,
    )
    // snapshotA + tool.call + snapshotB + done = 4
    const events = await collectSSEEvents(res, 4)
    const chunks = userDataEvents(events)

    expect(chunks).toHaveLength(3)

    // Series A snapshot
    expect(chunks[0]!.seriesSnapshot).toBe(true)
    expect(chunks[0]!.seriesId).toBe('sA')
    expect((chunks[0]!.data as Record<string, unknown>).delta).toBe('A1A2')

    // Non-series event preserved
    expect(chunks[1]!.type).toBe('tool.call')
    expect(chunks[1]!.seriesSnapshot).toBeUndefined()
    expect((chunks[1]!.data as Record<string, unknown>).tool).toBe('search')

    // Series B snapshot
    expect(chunks[2]!.seriesSnapshot).toBe(true)
    expect(chunks[2]!.seriesId).toBe('sB')
    expect((chunks[2]!.data as Record<string, unknown>).delta).toBe('B1B2')
  })

  // ─── 8. Mixed subscribers ──────────────────────────────────────────────────
  // Two concurrent SSE connections: one delta, one accumulated.
  // Both should get the correct format for live events.
  it('concurrent delta and accumulated subscribers receive correct formats', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Schedule publishing after SSE connections are established
    setTimeout(async () => {
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: 'X' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.publishEvent(task.id, {
        type: 'llm.chunk', level: 'info',
        data: { delta: 'Y' }, seriesId: 's1', seriesMode: 'accumulate',
      })
      await engine.transitionTask(task.id, 'completed')
    }, 100)

    // Open two concurrent SSE connections (both need includeStatus=true for stream close)
    // Start both requests without awaiting, then collect events
    const deltaPromise = app.request(
      `/tasks/${task.id}/events?seriesFormat=delta`,
    )
    const accPromise = app.request(
      `/tasks/${task.id}/events?seriesFormat=accumulated`,
    )

    const [deltaRes, accRes] = await Promise.all([deltaPromise, accPromise])

    // Delta subscriber:
    // History: status(running) = 1
    // Live: 2 chunks + status(completed) + done = 4
    // Total: 5
    const deltaEvents = await collectSSEEvents(deltaRes, 5)
    const deltaChunks = userDataEvents(deltaEvents)
    expect(deltaChunks).toHaveLength(2)
    expect((deltaChunks[0]!.data as Record<string, unknown>).delta).toBe('X')
    expect((deltaChunks[1]!.data as Record<string, unknown>).delta).toBe('Y')

    // Accumulated subscriber:
    const accEvents = await collectSSEEvents(accRes, 5)
    const accChunks = userDataEvents(accEvents)
    expect(accChunks).toHaveLength(2)
    expect((accChunks[0]!.data as Record<string, unknown>).delta).toBe('X')
    expect((accChunks[1]!.data as Record<string, unknown>).delta).toBe('XY')
  }, 10000)
})
