import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { GenericContainer, Wait, postgres, RetentionRuntime, until } from './helpers/retention-runtime.js'
import { PostgresLongTermStore } from '../../packages/postgres/src/long-term.js'

let pg: Awaited<ReturnType<InstanceType<typeof GenericContainer>['start']>>
let redis: typeof pg
let admin: ReturnType<typeof postgres>
const runtimes: RetentionRuntime[] = []
const databases: ReturnType<typeof postgres>[] = []
beforeAll(async () => {
  pg = await new GenericContainer('postgres:16-alpine').withEnvironment({ POSTGRES_USER: 'test', POSTGRES_PASSWORD: 'test', POSTGRES_DB: 'retention_test' })
    .withExposedPorts(5432).withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2)).start()
  redis = await new GenericContainer('redis:7-alpine').withExposedPorts(6379).withWaitStrategy(Wait.forLogMessage('Ready to accept connections')).start()
  admin = postgres(`postgres://test:test@127.0.0.1:${pg.getMappedPort(5432)}/retention_test`)
  for (const runtime of ['node', 'rust'] as const) {
    await admin.unsafe(`CREATE DATABASE ${runtime}_retention_test`)
    const url = `postgres://test:test@127.0.0.1:${pg.getMappedPort(5432)}/${runtime}_retention_test`
    runtimes.push(new RetentionRuntime(runtime, `redis://127.0.0.1:${redis.getMappedPort(6379)}`, url))
    databases.push(postgres(url, { onnotice: () => {} }))
  }
}, 120_000)
afterAll(async () => {
  for (const runtime of runtimes) await runtime.dispose()
  for (const sql of databases) await sql.end()
  await admin?.end()
  await redis?.stop()
  await pg?.stop()
}, 120_000)

function parseEvents(body: string) {
  return body.split('\n\n').filter(frame => frame.includes('event: taskcast.event')).map(frame => JSON.parse(frame.split('\n').find(line => line.startsWith('data: '))!.slice(6)))
}

describe('real Redis/PostgreSQL terminal retention parity', () => {
  it.each(['node', 'rust'] as const)('%s preserves facts, compacts notifications and resumes cleanup', async name => {
    const index = name === 'node' ? 0 : 1
    const runtime = runtimes[index]!
    const sql = databases[index]!
    const create = async (id: string, extra = {}) => {
      const response = await runtime.request('/tasks', { id, type: 'influagent.search.youtube', ...extra })
      expect(response.status, await response.clone().text()).toBe(201)
      return response.json()
    }
    const transition = async (id: string, status: string) => {
      const response = await runtime.request(`/tasks/${id}/status`, { status, result: { kept: true } }, 'PATCH')
      expect(response.status, await response.clone().text()).toBe(200)
    }
    await runtime.start(false)
    await create('legacy', { cleanup: { rules: [{ target: 'all', trigger: {} }] } })
    await transition('legacy', 'cancelled')
    await runtime.start(true, 60_000)
    await create('snapshot')
    await transition('snapshot', 'cancelled')
    await runtime.start(true)
    await create('empty', { cleanup: { rules: [] } })
    await transition('empty', 'cancelled')
    for (const status of ['failed', 'timeout']) { await create(status); await transition(status, 'running'); await transition(status, status) }
    const task = await create('notifications', { cleanup: { rules: [{ target: 'events', trigger: { afterMs: 2_000 } }] } })
    expect(task.cleanupPolicyVersion).toBe(1)
    await transition('notifications', 'running')
    const live = fetch(runtime.baseUrl + '/tasks/notifications/events').then(response => response.text())
    void live.catch(() => {}) // The awaiting assertion below still reports any stream failure.
    await until(async () => (await (await runtime.request('/tasks/notifications')).json()).subscriberCount === 1)
    const notifications = []
    for (let revision = 1; revision <= 3; revision++) for (const entityId of ['a', 'b']) {
      notifications.push({ type: 'search.candidate_changed', level: 'info', seriesId: `execution:search.candidate_changed:entity:${entityId}`, seriesMode: 'latest', data: { entityId, revision } })
    }
    for (const event of notifications) expect((await runtime.request('/tasks/notifications/events', event)).status).toBe(201)
    for (const delta of ['A', 'B']) expect((await runtime.request('/tasks/notifications/events', { type: 'output', level: 'info', seriesId: 'text', seriesMode: 'accumulate', data: { delta } })).status).toBe(201)
    expect((await runtime.request('/tasks/notifications/events', { type: 'audit', level: 'info', data: { kept: true } })).status).toBe(201)
    await transition('notifications', 'completed')
    const liveEvents = parseEvents(await live)
    expect(liveEvents.filter(event => event.type === 'search.candidate_changed').map(event => event.data)).toEqual(notifications.map(event => event.data))
    const history = await (await runtime.request('/tasks/notifications/events/history?seriesFormat=accumulated')).json()
    expect(history.filter((event: any) => event.type === 'search.candidate_changed').map((event: any) => event.data)).toEqual([{ entityId: 'a', revision: 3 }, { entityId: 'b', revision: 3 }])
    expect(history.find((event: any) => event.type === 'output').data.delta).toBe('AB')
    expect(history.some((event: any) => event.type === 'audit')).toBe(true)
    const archiveResponse = await runtime.request('/tasks/notifications/archive')
    expect(archiveResponse.status).toBe(200)
    const archive = await archiveResponse.json()
    const [{ creation_token: oldToken }] = await sql`SELECT creation_token FROM taskcast_tasks WHERE id = 'notifications'`
    await until(async () => (await (await runtime.request('/tasks/notifications')).json()).historyExpiredAt > 0)
    const expired = await runtime.request('/tasks/notifications/events/history?since.id=old')
    expect(expired.headers.get('X-Taskcast-History-Expired')).toBe('true')
    expect(await expired.json()).toEqual([])
    const sse = await (await runtime.request('/tasks/notifications/events?since.index=0')).text()
    expect(sse.indexOf('taskcast.history_expired')).toBeGreaterThanOrEqual(0)
    expect(sse.indexOf('taskcast.history_expired')).toBeLessThan(sse.indexOf('taskcast.done'))
    expect(sse).toContain('"reason":"completed"')
    expect(sse).not.toContain('event: taskcast.event')
    const exportExpired = await runtime.request('/tasks/notifications/archive')
    expect(exportExpired.status).toBe(409)
    expect((await exportExpired.json()).code).toBe('TASKCAST_HISTORY_EXPIRED')
    expect((await runtime.request('/tasks/notifications/events', { type: 'late', level: 'info', data: {} })).status).toBe(400)
    const store = new PostgresLongTermStore(sql)
    await expect(store.saveEvent({ id: `${name}-late`, taskId: 'notifications', index: 30, timestamp: Date.now(), type: 'late', level: 'info', data: {} }, { creationToken: oldToken })).rejects.toThrow()
    expect((await runtime.request('/tasks/import', { archive })).status).toBe(409)
    expect((await runtime.request('/tasks/import', { archive, overwrite: true })).status).toBe(200)
    const restored = await (await runtime.request('/tasks/notifications')).json()
    expect(restored.historyExpiredAt).toBeUndefined()
    expect(restored.cleanupPolicyVersion).toBeUndefined()
    expect((await runtime.request('/tasks/notifications/archive')).status).toBe(200)
    await expect(store.saveTask(archive.task, { creationToken: oldToken })).rejects.toThrow()

    await create('all', { cleanup: { rules: [{ target: 'all', trigger: {} }] } })
    await transition('all', 'cancelled')
    await until(async () => (await runtime.request('/tasks/all')).status === 404)
    for (const suffix of ['/events/history', '/events', '/archive']) expect((await runtime.request('/tasks/all' + suffix)).status).toBe(404)

    await create('chunks', { cleanup: { rules: [{ target: 'events', trigger: {} }] } })
    const bulk = Array.from({ length: 1_500 }, (_, n) => ({ type: 'keep', level: 'info', data: n }))
    expect((await runtime.request('/tasks/chunks/events', bulk)).status).toBe(201)
    await transition('chunks', 'cancelled')
    await until(async () => (await (await runtime.request('/tasks/chunks')).json()).historyExpiredAt > 0)
    await runtime.stop()
    const [{ count }] = await sql`SELECT count(*)::int AS count FROM taskcast_events WHERE task_id = 'chunks'`
    expect(count).toBeGreaterThan(0)
    expect(count).toBeLessThanOrEqual(501)
    await runtime.start(true)
    await until(async () => Number((await sql`SELECT count(*) AS count FROM taskcast_events WHERE task_id = 'chunks'`)[0]!.count) === 0)
    for (const id of ['legacy', 'snapshot', 'empty', 'failed', 'timeout', 'notifications']) {
      const task = await (await runtime.request(`/tasks/${id}`)).json()
      expect(task.historyExpiredAt, id).toBeUndefined()
      expect((await (await runtime.request(`/tasks/${id}/events/history`)).json()).length, id).toBeGreaterThan(0)
    }
    expect((await (await runtime.request('/tasks/chunks')).json()).result).toEqual({ kept: true })
  }, 120_000)
})
