import { randomUUID } from 'node:crypto'
import type postgres from 'postgres'
import { cleanupDeadline, StorageFenceConflictError, StoragePreconditionError, type CleanupBatchResult, type CleanupClaim, type Task } from '@taskcast/core'

type Sql = ReturnType<typeof postgres>
const nowSql = (sql: Sql) => sql`FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT`

export function nextCleanupDeadline(task: Task): number | null {
  const events = task.historyExpiredAt === undefined ? cleanupDeadline(task, 'events') : null
  const all = cleanupDeadline(task, 'all')
  return events === null ? all : all === null ? events : Math.min(events, all)
}

function positive(value: number): void {
  if (!Number.isSafeInteger(value) || value <= 0 || value > 2_147_483_647) throw new StoragePreconditionError('Invalid cleanup batch or lease bound')
}

export class PostgresCleanupStore {
  constructor(private sql: Sql, private readTask: (row: postgres.Row) => Task) {}

  async claim(limit: number, ttl: number): Promise<CleanupClaim[]> {
    positive(limit); positive(ttl)
    return this.sql.begin(async connection => {
      const sql = connection as unknown as Sql
      const rows = await sql`
        SELECT *, ${nowSql(sql)} AS cleanup_now FROM taskcast_tasks
        WHERE cleanup_policy_version = 1 AND creation_token IS NOT NULL
          AND creation_completed_at IS NOT NULL
          AND status IN ('completed', 'failed', 'cancelled', 'timeout') AND completed_at IS NOT NULL
          AND cleanup_due_at <= ${nowSql(sql)}
          AND (cleanup_next_attempt_at IS NULL OR cleanup_next_attempt_at <= ${nowSql(sql)})
          AND (cleanup_claim_until IS NULL OR cleanup_claim_until <= ${nowSql(sql)})
        ORDER BY GREATEST(cleanup_due_at, COALESCE(cleanup_next_attempt_at, cleanup_due_at)), id LIMIT ${limit} FOR UPDATE SKIP LOCKED
      `
      const claims: CleanupClaim[] = []
      for (const row of rows) {
        const task = this.readTask(row)
        const allDue = cleanupDeadline(task, 'all')
        const target = row['cleanup_in_progress'] === true ? row['cleanup_target'] as CleanupClaim['target']
          : allDue !== null && allDue <= Number(row['cleanup_now']) ? 'all' : 'events'
        const claim: CleanupClaim = { taskId: task.id, creationToken: String(row['creation_token']), claimToken: randomUUID(), target, completedAt: task.completedAt!, taskVersion: Number(row['task_version']) }
        await sql`UPDATE taskcast_tasks SET cleanup_claim_token = ${claim.claimToken}, cleanup_claim_until = ${nowSql(sql)} + ${ttl}, cleanup_target = ${target} WHERE id = ${task.id}`
        claims.push(claim)
      }
      return claims
    })
  }

  async ready(claim: CleanupClaim): Promise<boolean> {
    return this.sql.begin(async connection => {
      const sql = connection as unknown as Sql
      return Boolean(await this.locked(sql, claim)) && await this.settled(sql, claim.taskId)
    })
  }

  async renew(claim: CleanupClaim, ttl: number): Promise<boolean> {
    positive(ttl)
    const sql = this.sql
    const rows = await sql`UPDATE taskcast_tasks SET cleanup_claim_until = ${nowSql(sql)} + ${ttl}
      WHERE id = ${claim.taskId} AND creation_token = ${claim.creationToken}
        AND cleanup_claim_token = ${claim.claimToken} AND cleanup_claim_until > ${nowSql(sql)} RETURNING id`
    return rows.length === 1
  }

  async defer(claim: CleanupClaim, retryAfterMs: number): Promise<void> {
    if (!Number.isSafeInteger(retryAfterMs) || retryAfterMs < 0 || retryAfterMs > 2_147_483_647) throw new StoragePreconditionError('Invalid cleanup retry delay')
    const sql = this.sql
    await sql`UPDATE taskcast_tasks SET cleanup_claim_token = NULL, cleanup_claim_until = NULL, cleanup_next_attempt_at = ${nowSql(sql)} + ${retryAfterMs}
      WHERE id = ${claim.taskId} AND creation_token = ${claim.creationToken} AND cleanup_claim_token = ${claim.claimToken}`
  }

  private async locked(sql: Sql, claim: CleanupClaim): Promise<postgres.Row | undefined> {
    const rows = await sql`SELECT * FROM taskcast_tasks
      WHERE id = ${claim.taskId} AND creation_token = ${claim.creationToken} AND cleanup_policy_version = 1
        AND cleanup_claim_token = ${claim.claimToken} AND cleanup_claim_until > ${nowSql(sql)}
        AND cleanup_target = ${claim.target} AND completed_at = ${claim.completedAt} AND task_version = ${claim.taskVersion}
        AND status IN ('completed', 'failed', 'cancelled', 'timeout')
      FOR UPDATE`
    return rows[0]
  }

  private async settled(sql: Sql, taskId: string): Promise<boolean> {
    const [row] = await sql`SELECT
      NOT EXISTS (SELECT 1 FROM taskcast_durable_assignments WHERE task_id = ${taskId})
      AND NOT EXISTS (SELECT 1 FROM taskcast_terminal_outbox WHERE task_id = ${taskId} AND projected_at IS NULL)
      AND NOT EXISTS (SELECT 1 FROM taskcast_archive_generations WHERE task_id = ${taskId} AND status = 'uploading') AS ready`
    return row?.['ready'] === true
  }

  async begin(claim: CleanupClaim, epoch: number, throughIndex: number): Promise<boolean> {
    positive(epoch)
    if (!Number.isSafeInteger(throughIndex) || throughIndex < -1) throw new StoragePreconditionError('Invalid cleanup watermark')
    return this.sql.begin(async connection => {
      const sql = connection as unknown as Sql
      const row = await this.locked(sql, claim)
      if (!row || row['storage_state'] !== 'cold' || Number(row['storage_epoch']) !== epoch
        || row['active_release_generation'] !== null || Number(row['archive_watermark']) !== throughIndex) return false
      const due = cleanupDeadline(this.readTask(row), claim.target)
      const [clock] = await sql`SELECT ${nowSql(sql)} AS now`
      if (due === null || due > Number(clock!['now']) || !await this.settled(sql, claim.taskId)) return false
      const [beyond] = await sql`SELECT EXISTS (SELECT 1 FROM taskcast_events WHERE task_id = ${claim.taskId} AND idx > ${throughIndex})
        OR EXISTS (SELECT 1 FROM taskcast_series_state WHERE task_id = ${claim.taskId} AND through_index > ${throughIndex}) AS present`
      if (beyond?.['present']) return false
      await sql`UPDATE taskcast_tasks SET cleanup_in_progress = true,
        history_expired_at = COALESCE(history_expired_at, ${nowSql(sql)}),
        history_expired_through_index = GREATEST(COALESCE(history_expired_through_index, -1), ${throughIndex})
        WHERE id = ${claim.taskId}`
      return true
    })
  }

  async deleteBatch(claim: CleanupClaim, limit: number): Promise<CleanupBatchResult> {
    positive(limit)
    return this.sql.begin(async connection => {
      const sql = connection as unknown as Sql
      const row = await this.locked(sql, claim)
      if (!row || row['cleanup_in_progress'] !== true || row['history_expired_at'] === null
        || row['storage_state'] !== 'cold' || !await this.settled(sql, claim.taskId)) {
        throw new StorageFenceConflictError('Cleanup claim or cold-state precondition was lost')
      }
      const deleted = await sql`DELETE FROM taskcast_events WHERE id IN (
        SELECT id FROM taskcast_events WHERE task_id = ${claim.taskId} ORDER BY idx, id LIMIT ${limit}
      ) RETURNING id`
      const [remaining] = await sql`SELECT EXISTS (SELECT 1 FROM taskcast_events WHERE task_id = ${claim.taskId}) AS present`
      if (remaining?.['present']) return { deletedEvents: deleted.length, complete: false }
      // Every relation is bounded, including payload-bearing series and old receipts.
      await sql`DELETE FROM taskcast_series_state WHERE (task_id, series_id) IN (
        SELECT task_id, series_id FROM taskcast_series_state WHERE task_id = ${claim.taskId} LIMIT ${limit})`
      await sql`DELETE FROM taskcast_archive_batches WHERE (task_id, generation, ordinal) IN (
        SELECT task_id, generation, ordinal FROM taskcast_archive_batches WHERE task_id = ${claim.taskId} LIMIT ${limit})`
      await sql`DELETE FROM taskcast_archive_generations g WHERE (task_id, generation) IN (
        SELECT task_id, generation FROM taskcast_archive_generations WHERE task_id = ${claim.taskId} LIMIT ${limit})
        AND NOT EXISTS (SELECT 1 FROM taskcast_archive_batches b WHERE b.task_id = g.task_id AND b.generation = g.generation)`
      await sql`DELETE FROM taskcast_terminal_outbox WHERE projection_id IN (
        SELECT projection_id FROM taskcast_terminal_outbox WHERE task_id = ${claim.taskId} AND projected_at IS NOT NULL LIMIT ${limit})`
      const [related] = await sql`SELECT
        EXISTS (SELECT 1 FROM taskcast_series_state WHERE task_id = ${claim.taskId})
        OR EXISTS (SELECT 1 FROM taskcast_archive_generations WHERE task_id = ${claim.taskId})
        OR EXISTS (SELECT 1 FROM taskcast_terminal_outbox WHERE task_id = ${claim.taskId}) AS present`
      if (related?.['present']) return { deletedEvents: deleted.length, complete: false }
      if (claim.target === 'all') {
        await sql`DELETE FROM taskcast_tasks WHERE id = ${claim.taskId}`
      } else {
        const nextDue = cleanupDeadline(this.readTask(row), 'all')
        await sql`UPDATE taskcast_tasks SET cleanup_in_progress = false, cleanup_target = NULL,
          cleanup_due_at = ${nextDue}, cleanup_next_attempt_at = NULL, cleanup_claim_token = NULL, cleanup_claim_until = NULL
          WHERE id = ${claim.taskId}`
      }
      return { deletedEvents: deleted.length, complete: true }
    })
  }
}
