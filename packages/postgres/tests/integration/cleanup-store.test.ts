import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { join } from 'node:path'
import { TaskEngine, MemoryShortTermStore, MemoryBroadcastProvider, type Task, type TaskEvent } from '@taskcast/core'
import { PostgresLongTermStore } from '../../src/long-term.js'
import { runMigrations } from '../../src/migration-runner.js'

let container: StartedTestContainer
let sql: ReturnType<typeof postgres>
let store: PostgresLongTermStore
beforeAll(async () => {
  container = await new GenericContainer('postgres:16-alpine').withEnvironment({ POSTGRES_USER: 'test', POSTGRES_PASSWORD: 'test', POSTGRES_DB: 'retention_test' })
    .withExposedPorts(5432).withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2)).start()
  sql = postgres(`postgres://test:test@127.0.0.1:${container.getMappedPort(5432)}/retention_test`, { onnotice: () => {} })
  await runMigrations(sql, join(import.meta.dirname, '../../../../migrations/postgres'))
  store = new PostgresLongTermStore(sql)
}, 120_000)
afterAll(async () => { await sql?.end(); await container?.stop() })
beforeEach(async () => { await sql`TRUNCATE taskcast_tasks CASCADE` })

const event = (taskId: string, index = 0): TaskEvent => ({ id: `${taskId}-${index}`, taskId, index, timestamp: 1_000 + index, type: 'test', level: 'info', data: { delta: 'x' } })
async function enrolled(id: string, target: 'events' | 'all' = 'events', count = 1) {
  const ctx = { creationToken: `${id}-generation` }
  const task: Task = { id, status: 'running', createdAt: 0, updatedAt: 0, cleanup: { rules: [{ target, trigger: { afterMs: 0 } }] }, cleanupPolicyVersion: 1, cleanupResolvedAt: 0 }
  expect(await store.claimTaskCreation(task, ctx.creationToken, 30_000)).toBe(true)
  await store.completeTaskCreation(id, ctx.creationToken)
  for (let n = 0; n < count; n++) await store.saveEvent(event(id, n), ctx)
  const completed: Task = { ...task, status: 'completed', completedAt: 1_000, updatedAt: 1_000, result: { value: 'keep' } }
  await store.saveTask(completed, ctx)
  // Adapter fixture: represent a successfully finalized cold archive; real release is tested by the coordinator suite.
  await sql`UPDATE taskcast_tasks SET storage_state = 'cold', cold_at = 1000, archive_watermark = ${count - 1} WHERE id = ${id}`
  return { task: completed, ctx }
}

describe('durable terminal cleanup', () => {
  it('coordinates a real archive release before marking and deleting durable history', async () => {
    const hot = new MemoryShortTermStore()
    const engine = new TaskEngine({ shortTermStore: hot, longTermStore: store, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [{ target: 'events', trigger: {} }] } })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'test', level: 'info', data: 'keep result' })
    await engine.transitionTask(task.id, 'completed', { result: { kept: true } })
    await vi.waitFor(async () => expect(await store.getEvents(task.id)).toHaveLength(3))
    expect(await engine.sweepCleanup()).toMatchObject({ completed: 1, deletedEvents: 3, failed: 0 })
    expect(await hot.getTask(task.id)).toBeNull()
    expect((await store.getTask(task.id))?.result).toEqual({ kept: true })
    expect((await store.getTask(task.id))?.historyExpiredAt).toBeGreaterThan(0)
    expect(await store.getLastEventIndex(task.id)).toBe(2)
    expect(await sql`SELECT * FROM taskcast_archive_batches WHERE task_id = ${task.id}`).toHaveLength(0)
  })

  it('does not enroll legacy cleanup JSON and does scan cold enrolled tasks', async () => {
    await store.saveTask({ id: 'legacy', status: 'completed', createdAt: 0, updatedAt: 0, completedAt: 1_000, cleanup: { rules: [{ target: 'all', trigger: {} }] } })
    await enrolled('new')
    const claims = await store.claimCleanupTasks(10, 30_000)
    expect(claims.map(c => c.taskId)).toEqual(['new'])
    expect(claims[0]?.creationToken).toBe('new-generation')
    expect((await store.getTask('new'))?.cleanupPolicyVersion).toBe(1)
  })

  it('bounds deletion, resumes after a process interruption and retains result and highest index', async () => {
    await enrolled('large', 'events', 1_501)
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.beginTaskCleanup(claim!, 1, 1_500)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(claim!, 1_000)).toEqual({ deletedEvents: 1_000, complete: false })
    expect((await store.getTask('large'))?.historyExpiredAt).toBeGreaterThan(0)
    await store.deferCleanupClaim(claim!, 0)
    store = new PostgresLongTermStore(sql)
    const [retry] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.deleteTaskCleanupBatch(retry!, 1_000)).toEqual({ deletedEvents: 501, complete: true })
    expect((await store.getTask('large'))?.result).toEqual({ value: 'keep' })
    expect(await store.getLastEventIndex('large')).toBe(1_500)
    expect(await store.claimCleanupTasks(1, 30_000)).toEqual([])
  })

  it('does not revive expired history through ordinary or series writes', async () => {
    const { ctx, task } = await enrolled('late')
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await store.beginTaskCleanup(claim!, 1, 0)
    await store.deleteTaskCleanupBatch(claim!, 1_000)
    await expect(store.saveEvent(event('late'), ctx)).rejects.toThrow()
    await expect(store.replaceLastSeriesEvent('late', 's', { ...event('late'), seriesId: 's', seriesMode: 'latest' }, ctx)).rejects.toThrow()
    await expect(store.accumulateSeries('late', 's', { ...event('late'), seriesId: 's', seriesMode: 'accumulate' }, 'delta', ctx)).rejects.toThrow()
    await expect(store.saveTask(task, ctx)).rejects.toThrow()
    expect(await sql`SELECT id FROM taskcast_events WHERE task_id = 'late'`).toHaveLength(0)
  })

  it('fences deletion and same-ID recreation from old task and event writes', async () => {
    const old = await enrolled('reuse', 'all')
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await store.beginTaskCleanup(claim!, 1, 0)
    expect((await store.deleteTaskCleanupBatch(claim!, 1_000)).complete).toBe(true)
    expect(await store.getTask('reuse')).toBeNull()
    await expect(store.saveTask(old.task, old.ctx)).rejects.toThrow()
    await expect(store.saveTask(old.task)).rejects.toThrow()
    const fresh = { ...old.task, status: 'running' as const, completedAt: undefined }
    await store.claimTaskCreation(fresh, 'fresh-generation', 30_000)
    await store.completeTaskCreation('reuse', 'fresh-generation')
    await expect(store.saveEvent(event('reuse'), old.ctx)).rejects.toThrow()
    await store.saveEvent(event('reuse', 1), { creationToken: 'fresh-generation' })
    expect((await store.getEvents('reuse')).map(e => e.index)).toEqual([1])
  })

  it('requires context and prevents overlapping or stolen cleanup claims', async () => {
    const { task, ctx } = await enrolled('claimed')
    await expect(store.saveTask(task)).rejects.toThrow()
    await expect(store.saveEvent(event('claimed', 2))).rejects.toThrow()
    const [left, right] = await Promise.all([store.claimCleanupTasks(1, 30_000), store.claimCleanupTasks(1, 30_000)])
    expect(left.length + right.length).toBe(1)
    const claim = [...left, ...right][0]!
    const stolen = { ...claim, claimToken: 'wrong' }
    expect(await store.beginTaskCleanup(stolen, 1, 0)).toBe(false)
    expect(await store.renewCleanupClaim(stolen, 30_000)).toBe(false)
    expect(await store.renewCleanupClaim(claim, 30_000)).toBe(true)
    expect(await store.beginTaskCleanup({ ...claim, taskVersion: claim.taskVersion + 1 }, 1, 0)).toBe(false)
    expect(await store.beginTaskCleanup(claim, 2, 0)).toBe(false)
    expect(await store.beginTaskCleanup(claim, 1, 1)).toBe(false)
    await expect(store.deleteTaskCleanupBatch(stolen, 100)).rejects.toThrow()
    await store.saveEvent(event('claimed', 2), ctx)
  })

  it('defers a blocked task so a later task can be claimed', async () => {
    await enrolled('a-blocked')
    await enrolled('b-ready')
    const [first] = await store.claimCleanupTasks(1, 30_000)
    expect(first?.taskId).toBe('a-blocked')
    await store.deferCleanupClaim(first!, 60_000)
    expect((await store.claimCleanupTasks(1, 30_000))[0]?.taskId).toBe('b-ready')
  })

  it('refuses deletion while assignments or projections remain unsettled', async () => {
    await enrolled('busy')
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await store.saveDurableAssignment({ taskId: 'busy', workerId: 'worker', cost: 1, assignedAt: 100, status: 'running' })
    expect(await store.beginTaskCleanup(claim!, 1, 0)).toBe(false)
    expect(await store.canCleanupTask(claim!)).toBe(false)
    await store.deleteDurableAssignment('busy')
    expect(await store.canCleanupTask(claim!)).toBe(true)
    // Pending outbox fixture exercises the cleanup dependency guard, not normal terminalization.
    await sql`INSERT INTO taskcast_terminal_outbox(projection_id, task_id, event_id, payload, created_at) VALUES ('p', 'busy', 'e', '{}'::jsonb, 0)`
    expect(await store.beginTaskCleanup(claim!, 1, 0)).toBe(false)
    await sql`UPDATE taskcast_terminal_outbox SET projected_at = 1 WHERE projection_id = 'p'`
    await sql`INSERT INTO taskcast_archive_generations(task_id,generation,storage_epoch,target_watermark,manifest,status,created_at,updated_at) VALUES ('busy','archive',1,0,'{}'::jsonb,'uploading',0,0)`
    expect(await store.beginTaskCleanup(claim!, 1, 0)).toBe(false)
    await sql`UPDATE taskcast_archive_generations SET status = 'finalized' WHERE task_id = 'busy'`
    expect(await store.beginTaskCleanup(claim!, 1, 0)).toBe(true)
    await store.deleteTaskCleanupBatch(claim!, 1_000)
    expect(await sql`SELECT * FROM taskcast_terminal_outbox WHERE task_id = 'busy'`).toHaveLength(0)
    expect(await sql`SELECT * FROM taskcast_archive_generations WHERE task_id = 'busy'`).toHaveLength(0)
  })

  it('bounds removal of payload-bearing series state as well as events', async () => {
    const ctx = { creationToken: 'series-generation' }
    const task: Task = { id: 'series', status: 'running', createdAt: 0, updatedAt: 0, cleanupPolicyVersion: 1, cleanupResolvedAt: 0, cleanup: { rules: [{ target: 'events', trigger: {} }] } }
    await store.claimTaskCreation(task, ctx.creationToken, 30_000)
    await store.completeTaskCreation(task.id, ctx.creationToken)
    await store.replaceLastSeriesEvent(task.id, 'latest', { ...event(task.id, 0), seriesId: 'latest', seriesMode: 'latest' }, ctx)
    await store.replaceLastSeriesEvent(task.id, 'latest', { ...event(task.id, 1), seriesId: 'latest', seriesMode: 'latest' }, ctx)
    await store.accumulateSeries(task.id, 'sum', { ...event(task.id, 2), seriesId: 'sum', seriesMode: 'accumulate' }, 'delta', ctx)
    await store.accumulateSeries(task.id, 'sum', { ...event(task.id, 3), seriesId: 'sum', seriesMode: 'accumulate' }, 'delta', ctx)
    await store.saveTask({ ...task, status: 'completed', completedAt: 1000 }, ctx)
    await sql`UPDATE taskcast_tasks SET storage_state = 'cold', archive_watermark = 3 WHERE id = 'series'`
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.beginTaskCleanup(claim!, 1, 3)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 1, complete: false })
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 1, complete: false })
    expect(await sql`SELECT * FROM taskcast_series_state WHERE task_id = 'series'`).toHaveLength(1)
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 0, complete: true })
    expect(await sql`SELECT * FROM taskcast_series_state WHERE task_id = 'series'`).toHaveLength(0)
    expect(await store.getLastEventIndex('series')).toBe(3)
  })

  it('rolls back a failed delete batch and rejects a lost claim before retrying', async () => {
    await enrolled('rollback')
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await store.beginTaskCleanup(claim!, 1, 0)
    await sql.unsafe(`CREATE FUNCTION cleanup_test_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected delete failure'; END; $$`)
    await sql.unsafe(`CREATE TRIGGER cleanup_test_failure AFTER DELETE ON taskcast_events FOR EACH STATEMENT EXECUTE FUNCTION cleanup_test_failure()`)
    try {
      await expect(store.deleteTaskCleanupBatch(claim!, 1000)).rejects.toThrow('injected delete failure')
      expect(await sql`SELECT id FROM taskcast_events WHERE task_id = 'rollback'`).toHaveLength(1)
      expect((await store.getTask('rollback'))?.historyExpiredAt).toBeGreaterThan(0)
    } finally {
      await sql.unsafe('DROP TRIGGER cleanup_test_failure ON taskcast_events')
      await sql.unsafe('DROP FUNCTION cleanup_test_failure()')
    }
    await sql`UPDATE taskcast_tasks SET cleanup_claim_until = 0 WHERE id = 'rollback'`
    await expect(store.deleteTaskCleanupBatch(claim!, 1000)).rejects.toThrow()
    const [retry] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.deleteTaskCleanupBatch(retry!, 1000)).toEqual({ deletedEvents: 1, complete: true })
  })

  it('rejects unsafe bounds without querying or deleting', async () => {
    await expect(store.claimCleanupTasks(0, 1)).rejects.toThrow()
    await expect(store.claimCleanupTasks(1, 0)).rejects.toThrow()
    await enrolled('bounds')
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await expect(store.deleteTaskCleanupBatch(claim!, -1)).rejects.toThrow()
  })
})


it('explicit overwrite restores expired history with a new generation and no automatic enrollment', async () => {
  const hot = new MemoryShortTermStore()
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: store, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [{ target: 'events', trigger: {} }] } })
  await engine.createTask({ id: 'restore' })
  await engine.transitionTask('restore', 'running')
  await engine.transitionTask('restore', 'completed')
  await vi.waitFor(async () => expect(await store.getEvents('restore')).toHaveLength(2))
  const old = (await store.getTaskStorageMetadata('restore'))!
  const archive = await engine.exportTaskArchive('restore')
  expect(await engine.sweepCleanup()).toMatchObject({ completed: 1 })
  await expect(engine.importTaskArchive(archive)).rejects.toThrow(/exists/)
  archive.task.historyExpiredAt = 1 // untrusted server markers never enroll restored archives
  await engine.importTaskArchive(archive, { overwrite: true })
  expect((await engine.getTask('restore'))).toMatchObject({ status: 'completed', cleanup: archive.task.cleanup })
  expect((await engine.getTask('restore'))?.cleanupPolicyVersion).toBeUndefined()
  expect((await engine.getTask('restore'))?.historyExpiredAt).toBeUndefined()
  expect((await store.getTaskStorageMetadata('restore'))?.creationToken).not.toBe(old.creationToken)
  await expect(store.saveEvent(event('restore', 2), { creationToken: old.creationToken! })).rejects.toThrow()
  expect(await engine.getEvents('restore')).toHaveLength(2)
  expect(await engine.sweepCleanup()).toMatchObject({ claimed: 0 })
})
