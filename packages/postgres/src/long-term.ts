import type postgres from 'postgres'
import {
  DependencyUnavailableError,
  StorageIntegrityError,
  archiveEventRecord,
  canonicalJson,
  computeArchiveBatchDigest,
  computeArchiveSourceDigest,
  computeArchiveSourcePageDigest,
  computeSeriesStateDigest,
  durableSeriesStateRecord,
  type ArchiveBatch,
  type ArchiveBatchReceipt,
  type ArchiveGeneration,
  type ArchiveSourceManifest,
  type AssignMode,
  type CleanupRule,
  type DependencyObserver,
  type DisconnectPolicy,
  type DurableSeriesState,
  type EventQueryOptions,
  type LongTermStore,
  type SeriesMode,
  type Task,
  type TaskArchiveImportOptions,
  type TaskArchiveRestoreData,
  type TaskAuthConfig,
  type TaskError,
  type TaskEvent,
  type TaskStorageMetadata,
  type TaskStorageMetadataCas,
  type StorageReleaseRequest,
  type WebhookConfig,
  type WorkerAuditEvent,
} from '@taskcast/core'
import { classifyPostgresConnectivity } from './health.js'

const TASKS = 'taskcast_tasks'
const EVENTS = 'taskcast_events'
const WORKER_EVENTS = 'taskcast_worker_events'
const ARCHIVE_GENERATIONS = 'taskcast_archive_generations'
const ARCHIVE_BATCHES = 'taskcast_archive_batches'
const SERIES_STATE = 'taskcast_series_state'
const POSTGRES_INTEGER_MAX = 2_147_483_647

type PostgresClient = ReturnType<typeof postgres>
type EventConflictMode = 'ignore' | 'strict'
interface ArchiveSeriesCoverage {
  seriesId: string
  mode: DurableSeriesState['mode']
  throughIndex: number
}

export class PostgresLongTermStore implements LongTermStore {
  readonly supportsHotColdRelease = true

  constructor(
    private sql: ReturnType<typeof postgres>,
    private observer?: DependencyObserver,
  ) {}

  private async observed<T>(operation: () => Promise<T>): Promise<T> {
    try {
      const result = await operation()
      this.observer?.observe({ dependency: 'postgres', state: 'healthy' })
      return result
    } catch (error) {
      const kind = classifyPostgresConnectivity(error)
      if (!kind) throw error
      this.observer?.observe({
        dependency: 'postgres',
        state: 'unhealthy',
        errorKind: kind,
      })
      throw new DependencyUnavailableError('postgres', kind, error)
    }
  }

  async saveTask(task: Task): Promise<void> {
    return this.observed(async () => {
      await this.saveTaskWithClient(this.sql, task)
    })
  }

  async createTaskIfAbsent(task: Task): Promise<boolean> {
    return this.insertTaskIfAbsent(task, null)
  }

  async claimTaskCreation(
    task: Task,
    creationToken: string,
    claimTtlMs: number,
  ): Promise<boolean> {
    if (claimTtlMs <= 0) {
      throw new StorageIntegrityError('Creation claim TTL must be positive')
    }
    const t = TASKS
    const e = EVENTS
    const rows = await this.sql`
      INSERT INTO ${this.sql(t)} (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
        tags, assign_mode, cost, assigned_worker, disconnect_policy,
        creation_token, creation_claimed_at, creation_claim_expires_at,
        creation_completed_at
      ) VALUES (
        ${task.id}, ${task.type ?? null}, ${task.status},
        ${task.params ? this.sql.json(task.params as never) : null},
        ${task.result ? this.sql.json(task.result as never) : null},
        ${task.error ? this.sql.json(task.error as never) : null},
        ${task.metadata ? this.sql.json(task.metadata as never) : null},
        ${task.authConfig ? this.sql.json(task.authConfig as never) : null},
        ${task.webhooks ? this.sql.json(task.webhooks as never) : null},
        ${task.cleanup ? this.sql.json(task.cleanup as never) : null},
        ${task.createdAt}, ${task.updatedAt},
        ${task.completedAt ?? null}, ${task.ttl ?? null},
        ${task.tags ? this.sql.json(task.tags as never) : null},
        ${task.assignMode ?? null},
        ${task.cost ?? null},
        ${task.assignedWorker ?? null},
        ${task.disconnectPolicy ?? null},
        ${creationToken},
        FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT,
        FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT + ${claimTtlMs},
        NULL
      )
      ON CONFLICT (id) DO UPDATE SET
        type = EXCLUDED.type,
        status = EXCLUDED.status,
        params = EXCLUDED.params,
        result = EXCLUDED.result,
        error = EXCLUDED.error,
        metadata = EXCLUDED.metadata,
        auth_config = EXCLUDED.auth_config,
        webhooks = EXCLUDED.webhooks,
        cleanup = EXCLUDED.cleanup,
        created_at = EXCLUDED.created_at,
        updated_at = EXCLUDED.updated_at,
        completed_at = EXCLUDED.completed_at,
        ttl = EXCLUDED.ttl,
        tags = EXCLUDED.tags,
        assign_mode = EXCLUDED.assign_mode,
        cost = EXCLUDED.cost,
        assigned_worker = EXCLUDED.assigned_worker,
        disconnect_policy = EXCLUDED.disconnect_policy,
        creation_token = EXCLUDED.creation_token,
        creation_claimed_at = EXCLUDED.creation_claimed_at,
        creation_claim_expires_at = EXCLUDED.creation_claim_expires_at,
        creation_completed_at = NULL
      WHERE ${this.sql(t)}.creation_token IS NOT NULL
        AND ${this.sql(t)}.creation_completed_at IS NULL
        AND (
          ${this.sql(t)}.creation_claim_expires_at IS NULL
          OR ${this.sql(t)}.creation_claim_expires_at <=
            FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
        )
        AND ${this.sql(t)}.status = 'pending'
        AND ${this.sql(t)}.updated_at = ${this.sql(t)}.created_at
        AND ${this.sql(t)}.result IS NULL
        AND ${this.sql(t)}.error IS NULL
        AND ${this.sql(t)}.completed_at IS NULL
        AND ${this.sql(t)}.storage_state = 'hot'
        AND ${this.sql(t)}.storage_epoch = 1
        AND ${this.sql(t)}.active_release_generation IS NULL
        AND ${this.sql(t)}.archive_watermark = -1
        AND ${this.sql(t)}.last_event_at IS NULL
        AND ${this.sql(t)}.cold_at IS NULL
        AND ${this.sql(t)}.task_version = 0
        AND NOT EXISTS (
          SELECT 1 FROM ${this.sql(e)} AS event
          WHERE event.task_id = ${this.sql(t)}.id
        )
      RETURNING id
    `
    return rows.length === 1
  }

  async completeTaskCreation(taskId: string, creationToken: string): Promise<boolean> {
    const t = TASKS
    const rows = await this.sql`
      UPDATE ${this.sql(t)}
      SET creation_completed_at = COALESCE(
            creation_completed_at,
            FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
          ),
          creation_claim_expires_at = NULL
      WHERE id = ${taskId}
        AND creation_token = ${creationToken}
      RETURNING id
    `
    return rows.length === 1
  }

  async abortTaskCreation(taskId: string, creationToken: string): Promise<boolean> {
    const t = TASKS
    const e = EVENTS
    const rows = await this.sql`
      DELETE FROM ${this.sql(t)} AS task
      WHERE task.id = ${taskId}
        AND task.creation_token = ${creationToken}
        AND task.creation_completed_at IS NULL
        AND task.status = 'pending'
        AND task.updated_at = task.created_at
        AND task.result IS NULL
        AND task.error IS NULL
        AND task.completed_at IS NULL
        AND task.storage_state = 'hot'
        AND task.storage_epoch = 1
        AND task.active_release_generation IS NULL
        AND task.archive_watermark = -1
        AND task.last_event_at IS NULL
        AND task.cold_at IS NULL
        AND task.task_version = 0
        AND NOT EXISTS (
          SELECT 1 FROM ${this.sql(e)} AS event WHERE event.task_id = task.id
        )
      RETURNING task.id
    `
    return rows.length === 1
  }

  private async insertTaskIfAbsent(
    task: Task,
    creationToken: string | null,
  ): Promise<boolean> {
    const t = TASKS
    const rows = await this.sql`
      INSERT INTO ${this.sql(t)} (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
        tags, assign_mode, cost, assigned_worker, disconnect_policy, creation_token
      ) VALUES (
        ${task.id}, ${task.type ?? null}, ${task.status},
        ${task.params ? this.sql.json(task.params as never) : null},
        ${task.result ? this.sql.json(task.result as never) : null},
        ${task.error ? this.sql.json(task.error as never) : null},
        ${task.metadata ? this.sql.json(task.metadata as never) : null},
        ${task.authConfig ? this.sql.json(task.authConfig as never) : null},
        ${task.webhooks ? this.sql.json(task.webhooks as never) : null},
        ${task.cleanup ? this.sql.json(task.cleanup as never) : null},
        ${task.createdAt}, ${task.updatedAt},
        ${task.completedAt ?? null}, ${task.ttl ?? null},
        ${task.tags ? this.sql.json(task.tags as never) : null},
        ${task.assignMode ?? null},
        ${task.cost ?? null},
        ${task.assignedWorker ?? null},
        ${task.disconnectPolicy ?? null},
        ${creationToken}
      )
      ON CONFLICT (id) DO NOTHING
      RETURNING id
    `
    return rows.length === 1
  }

  private async saveTaskWithClient(sql: PostgresClient, task: Task): Promise<void> {
    const t = TASKS
    await sql`
      INSERT INTO ${sql(t)} (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
        tags, assign_mode, cost, assigned_worker, disconnect_policy
      ) VALUES (
        ${task.id}, ${task.type ?? null}, ${task.status},
        ${task.params ? sql.json(task.params as never) : null},
        ${task.result ? sql.json(task.result as never) : null},
        ${task.error ? sql.json(task.error as never) : null},
        ${task.metadata ? sql.json(task.metadata as never) : null},
        ${task.authConfig ? sql.json(task.authConfig as never) : null},
        ${task.webhooks ? sql.json(task.webhooks as never) : null},
        ${task.cleanup ? sql.json(task.cleanup as never) : null},
        ${task.createdAt}, ${task.updatedAt},
        ${task.completedAt ?? null}, ${task.ttl ?? null},
        ${task.tags ? sql.json(task.tags as never) : null},
        ${task.assignMode ?? null},
        ${task.cost ?? null},
        ${task.assignedWorker ?? null},
        ${task.disconnectPolicy ?? null}
      )
      ON CONFLICT (id) DO UPDATE SET
        status = EXCLUDED.status,
        result = EXCLUDED.result,
        error = EXCLUDED.error,
        metadata = EXCLUDED.metadata,
        updated_at = EXCLUDED.updated_at,
        completed_at = EXCLUDED.completed_at,
        tags = EXCLUDED.tags,
        assign_mode = EXCLUDED.assign_mode,
        cost = EXCLUDED.cost,
        assigned_worker = EXCLUDED.assigned_worker,
        disconnect_policy = EXCLUDED.disconnect_policy
    `
  }

  async getTask(taskId: string): Promise<Task | null> {
    return this.observed(async () => {
      const t = TASKS
      const rows = await this.sql`
        SELECT * FROM ${this.sql(t)} WHERE id = ${taskId}
      `
      const row = rows[0]
      if (!row) return null
      return this._rowToTask(row)
    })
  }

  async saveEvent(event: TaskEvent): Promise<void> {
    return this.observed(async () => {
      await this.saveEventWithClient(this.sql, event, 'ignore')
    })
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    return this.observed(async () => {
      await this.sql.begin(async (sql) => {
        const tx = sql as unknown as PostgresClient
        const archiveWatermark = await this.lockTaskForSeriesWrite(tx, taskId)
        const committed = await this.getSeriesStateForUpdate(tx, taskId, seriesId)
        if (archiveWatermark >= event.index || (committed?.throughIndex ?? -1) >= event.index) {
          if (archiveWatermark >= event.index && !committed) {
            throw new StorageIntegrityError(
              `Archived latest series state is missing for ${taskId}:${seriesId}`,
            )
          }
          return
        }
        if (
          committed &&
          (committed.mode !== 'latest' ||
            committed.event.seriesAccField !== event.seriesAccField)
        ) {
          throw new StorageIntegrityError(
            `Durable series semantics conflict for ${taskId}:${seriesId}`,
          )
        }
        const existingEvents = await this.getSeriesEventsWithClient(tx, taskId, seriesId, 'latest')
        const first = existingEvents[0]

        if (!first) {
          await this.saveEventWithClient(tx, event, 'ignore')
        } else {
          await this.updateStoredSeriesEventWithClient(tx, first, event)
          await this.deleteDuplicateSeriesEventsWithClient(tx, taskId, seriesId, 'latest', first.id)
        }

        await this.saveSeriesStateWithClient(tx, {
          taskId,
          seriesId,
          mode: 'latest',
          event,
          throughIndex: event.index,
        })
      })
    })
  }

  async accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent> {
    return this.observed(async () => {
      return this.sql.begin(async (sql) => {
        const tx = sql as unknown as PostgresClient
        const archiveWatermark = await this.lockTaskForSeriesWrite(tx, taskId)
        const committed = await this.getSeriesStateForUpdate(tx, taskId, seriesId)
        if (archiveWatermark >= event.index || (committed?.throughIndex ?? -1) >= event.index) {
          if (!committed) {
            throw new StorageIntegrityError(
              `Archived accumulate series state is missing for ${taskId}:${seriesId}`,
            )
          }
          return committed.event
        }
        if (
          committed &&
          (committed.mode !== 'accumulate' ||
            (committed.event.seriesAccField ?? 'delta') !== field ||
            (event.seriesAccField ?? 'delta') !== field)
        ) {
          throw new StorageIntegrityError(
            `Durable series semantics conflict for ${taskId}:${seriesId}`,
          )
        }
        const existingEvents = await this.getSeriesEventsWithClient(tx, taskId, seriesId, 'accumulate')
        const first = existingEvents[0]
        const previous = existingEvents[existingEvents.length - 1]

        let accumulated = event
        if (previous) {
          const prevData = typeof previous.data === 'object' && previous.data !== null
            ? previous.data as Record<string, unknown>
            : {}
          const newData = typeof event.data === 'object' && event.data !== null
            ? event.data as Record<string, unknown>
            : {}
          if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
            accumulated = {
              ...event,
              data: { ...newData, [field]: prevData[field] + newData[field] },
            }
          }
        }

        if (!first) {
          await this.saveEventWithClient(tx, accumulated, 'ignore')
        } else {
          await this.updateStoredSeriesEventWithClient(tx, first, accumulated)
          await this.deleteDuplicateSeriesEventsWithClient(tx, taskId, seriesId, 'accumulate', first.id)
        }
        await this.saveSeriesStateWithClient(tx, {
          taskId,
          seriesId,
          mode: 'accumulate',
          event: accumulated,
          throughIndex: event.index,
        })

        return accumulated
      })
    })
  }

  private async lockTaskForSeriesWrite(
    sql: PostgresClient,
    taskId: string,
  ): Promise<number> {
    const rows = await sql`
      SELECT archive_watermark
      FROM ${sql(TASKS)}
      WHERE id = ${taskId}
      FOR UPDATE
    `
    if (!rows[0]) {
      throw new StorageIntegrityError(`Series task does not exist: ${taskId}`)
    }
    return Number(rows[0]['archive_watermark'])
  }

  private async getSeriesStateForUpdate(
    sql: PostgresClient,
    taskId: string,
    seriesId: string,
  ): Promise<DurableSeriesState | undefined> {
    const rows = await sql`
      SELECT * FROM ${sql(SERIES_STATE)}
      WHERE task_id = ${taskId} AND series_id = ${seriesId}
      FOR UPDATE
    `
    const row = rows[0]
    if (!row) return undefined
    return {
      taskId: row['task_id'] as string,
      seriesId: row['series_id'] as string,
      mode: row['mode'] as DurableSeriesState['mode'],
      event: row['event'] as TaskEvent,
      throughIndex: Number(row['through_index']),
    }
  }

  private async saveSeriesStateWithClient(
    sql: PostgresClient,
    state: DurableSeriesState,
  ): Promise<void> {
    await sql`
      INSERT INTO ${sql(SERIES_STATE)} (
        task_id, series_id, mode, event, through_index, updated_at
      ) VALUES (
        ${state.taskId}, ${state.seriesId}, ${state.mode},
        ${sql.json(state.event as never)}, ${state.throughIndex}, ${Date.now()}
      )
      ON CONFLICT (task_id, series_id) DO UPDATE SET
        mode = EXCLUDED.mode,
        event = EXCLUDED.event,
        through_index = EXCLUDED.through_index,
        updated_at = EXCLUDED.updated_at
      WHERE ${sql(SERIES_STATE)}.through_index < EXCLUDED.through_index
    `
  }

  private async saveEventWithClient(
    sql: PostgresClient,
    event: TaskEvent,
    onConflict: EventConflictMode,
  ): Promise<void> {
    const t = EVENTS
    await sql`
      INSERT INTO ${sql(t)} (
        id, task_id, idx, timestamp, type, level, data, series_id, series_mode, series_acc_field
      ) VALUES (
        ${event.id}, ${event.taskId}, ${event.index}, ${event.timestamp},
        ${event.type}, ${event.level},
        ${event.data != null ? sql.json(event.data as never) : null},
        ${event.seriesId ?? null}, ${event.seriesMode ?? null},
        ${event.seriesAccField ?? null}
      )
      ${onConflict === 'ignore' ? sql`ON CONFLICT (id) DO NOTHING` : sql``}
    `
  }

  private async getSeriesEventsWithClient(
    sql: PostgresClient,
    taskId: string,
    seriesId: string,
    mode: SeriesMode,
  ): Promise<TaskEvent[]> {
    const rows = await sql`
      SELECT * FROM ${sql(EVENTS)}
      WHERE task_id = ${taskId}
        AND series_id = ${seriesId}
        AND series_mode = ${mode}
      ORDER BY idx ASC
    `
    return rows.map((row) => this._rowToEvent(row))
  }

  private async updateStoredSeriesEventWithClient(
    sql: PostgresClient,
    existing: TaskEvent,
    event: TaskEvent,
  ): Promise<void> {
    await sql`
      UPDATE ${sql(EVENTS)}
      SET timestamp = ${event.timestamp},
          type = ${event.type},
          level = ${event.level},
          data = ${event.data != null ? sql.json(event.data as never) : null},
          series_id = ${event.seriesId ?? null},
          series_mode = ${event.seriesMode ?? null},
          series_acc_field = ${event.seriesAccField ?? null}
      WHERE id = ${existing.id}
    `
  }

  private async deleteDuplicateSeriesEventsWithClient(
    sql: PostgresClient,
    taskId: string,
    seriesId: string,
    mode: SeriesMode,
    keepEventId: string,
  ): Promise<void> {
    await sql`
      DELETE FROM ${sql(EVENTS)}
      WHERE task_id = ${taskId}
        AND series_id = ${seriesId}
        AND series_mode = ${mode}
        AND id <> ${keepEventId}
    `
  }

  async validateTaskArchiveRestore(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<void> {
    return this.observed(async () => {
      await this.validateTaskArchiveRestoreWithClient(this.sql, data, options)
    })
  }

  private async validateTaskArchiveRestoreWithClient(
    sql: PostgresClient,
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<boolean> {
    const taskId = data.task.id
    const existing = await sql`SELECT id FROM ${sql(TASKS)} WHERE id = ${taskId}`
    if (existing.length > 0 && options?.overwrite !== true) {
      throw new Error(`Task already exists: ${taskId}`)
    }

    const eventIds = Array.from(new Set(data.events.map((event) => event.id)))
    for (const eventId of eventIds) {
      const conflict = await sql`
        SELECT id FROM ${sql(EVENTS)}
        WHERE task_id <> ${taskId} AND id = ${eventId}
        LIMIT 1
      `
      if (conflict.length > 0) {
        throw new Error(`Archive event id conflicts with another task: ${eventId}`)
      }
    }

    return existing.length > 0
  }

  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    return this.observed(async () => {
      return this.sql.begin(async (sql) => {
        const tx = sql as unknown as PostgresClient
        const taskId = data.task.id
        const overwritten = await this.validateTaskArchiveRestoreWithClient(tx, data, options)

        await tx`DELETE FROM ${tx(EVENTS)} WHERE task_id = ${taskId}`
        await tx`DELETE FROM ${tx(TASKS)} WHERE id = ${taskId}`
        await this.saveTaskWithClient(tx, data.task)
        for (const event of data.events) {
          await this.saveEventWithClient(tx, event, 'strict')
        }

        return { overwritten }
      })
    })
  }

  async getTaskStorageMetadata(taskId: string): Promise<TaskStorageMetadata | null> {
    const rows = await this.sql`
      SELECT id, storage_state, storage_epoch, active_release_generation,
             archive_watermark, last_event_at, cold_at, execution_deadline_at,
             task_version
      FROM ${this.sql(TASKS)}
      WHERE id = ${taskId}
    `
    return rows[0] ? rowToStorageMetadata(rows[0]) : null
  }

  async persistStorageReleaseRequest(request: StorageReleaseRequest): Promise<boolean> {
    validateStorageReleaseRequest(request)
    const rows = await this.sql`
      UPDATE ${this.sql(TASKS)}
      SET release_requested_at = ${request.requestedAt},
          release_expected_index = ${request.expectedLastEventIndex},
          release_inactive_since = ${request.inactiveSince}
      WHERE id = ${request.taskId}
      RETURNING id
    `
    return rows.length === 1
  }

  async clearStorageReleaseRequest(request: StorageReleaseRequest): Promise<boolean> {
    validateStorageReleaseRequest(request)
    const rows = await this.sql`
      UPDATE ${this.sql(TASKS)}
      SET release_requested_at = NULL,
          release_expected_index = NULL,
          release_inactive_since = NULL
      WHERE id = ${request.taskId}
        AND release_requested_at = ${request.requestedAt}
        AND release_expected_index = ${request.expectedLastEventIndex}
        AND release_inactive_since = ${request.inactiveSince}
      RETURNING id
    `
    return rows.length === 1
  }

  async listStorageReleaseRequests(limit: number): Promise<StorageReleaseRequest[]> {
    if (!Number.isSafeInteger(limit) || limit <= 0) {
      throw new StorageIntegrityError('Storage release request limit must be positive')
    }
    const rows = await this.sql`
      SELECT id, release_requested_at, release_expected_index, release_inactive_since
      FROM ${this.sql(TASKS)}
      WHERE release_requested_at IS NOT NULL
        AND release_expected_index IS NOT NULL
        AND release_inactive_since IS NOT NULL
      ORDER BY release_requested_at, id
      LIMIT ${limit}
    `
    return rows.map((row) => ({
      taskId: row['id'] as string,
      requestedAt: Number(row['release_requested_at']),
      expectedLastEventIndex: Number(row['release_expected_index']),
      inactiveSince: Number(row['release_inactive_since']),
    }))
  }

  async compareAndSetTaskStorageMetadata(update: TaskStorageMetadataCas): Promise<boolean> {
    validateStorageMetadataCas(update)
    const next = update.next
    const rows = await this.sql`
      UPDATE ${this.sql(TASKS)}
      SET storage_state = ${next.storageState},
          storage_epoch = ${next.storageEpoch},
          active_release_generation = ${next.activeReleaseGeneration},
          archive_watermark = ${next.archiveWatermark},
          last_event_at = ${next.lastEventAt},
          cold_at = ${next.coldAt},
          execution_deadline_at = ${next.executionDeadlineAt},
          task_version = ${next.taskVersion}
      WHERE id = ${update.taskId}
        AND storage_state = ${update.expectedStorageState}
        AND storage_epoch = ${update.expectedStorageEpoch}
        AND active_release_generation IS NOT DISTINCT FROM ${update.expectedReleaseGeneration}
        AND archive_watermark = ${next.archiveWatermark}
        AND task_version <= ${next.taskVersion}
      RETURNING id
    `
    return rows.length === 1
  }

  async beginArchive(generation: ArchiveGeneration): Promise<ArchiveGeneration> {
    validateArchiveManifest(generation.manifest)
    if (!Number.isSafeInteger(generation.storageEpoch) || generation.storageEpoch < 1) {
      throw new StorageIntegrityError('Archive generation storage epoch is invalid')
    }
    if (generation.status !== 'open') {
      throw new StorageIntegrityError('A new archive generation must start open')
    }

    return this.sql.begin(async (sql) => {
      const tx = sql as unknown as PostgresClient
      const taskRows = await tx`
        SELECT storage_state, storage_epoch, active_release_generation, archive_watermark
        FROM ${tx(TASKS)}
        WHERE id = ${generation.taskId}
        FOR UPDATE
      `
      const task = taskRows[0]
      if (!task) throw new StorageIntegrityError(`Archive task does not exist: ${generation.taskId}`)

      const existingRows = await tx`
        SELECT *
        FROM ${tx(ARCHIVE_GENERATIONS)}
        WHERE task_id = ${generation.taskId}
          AND generation = ${generation.generation}
        FOR UPDATE
      `
      const existing = existingRows[0]
      if (existing) {
        const restored = rowToArchiveGeneration(existing)
        if (
          restored.storageEpoch !== generation.storageEpoch ||
          restored.targetWatermark !== generation.targetWatermark ||
          canonicalJson(restored.manifest) !== canonicalJson(generation.manifest)
        ) {
          throw new StorageIntegrityError('Archive generation replay conflicts with durable manifest')
        }
        return restored
      }

      if (
        task['storage_state'] !== 'releasing' ||
        Number(task['storage_epoch']) !== generation.storageEpoch ||
        task['active_release_generation'] !== generation.generation
      ) {
        throw new StorageIntegrityError('Archive generation is stale for the active task release')
      }
      if (
        generation.manifest.targetWatermark !== generation.targetWatermark ||
        generation.manifest.priorWatermark !== Number(task['archive_watermark']) ||
        generation.targetWatermark < Number(task['archive_watermark'])
      ) {
        throw new StorageIntegrityError('Archive manifest watermark conflicts with durable task state')
      }

      await tx`
        INSERT INTO ${tx(ARCHIVE_GENERATIONS)} (
          task_id, generation, storage_epoch, target_watermark, manifest,
          status, created_at, updated_at
        ) VALUES (
          ${generation.taskId}, ${generation.generation}, ${generation.storageEpoch},
          ${generation.targetWatermark}, ${tx.json(generation.manifest as never)},
          'uploading', ${generation.createdAt}, ${generation.updatedAt}
        )
      `
      return generation
    })
  }

  async archiveBatch(
    taskId: string,
    generation: string,
    batch: ArchiveBatch,
  ): Promise<ArchiveBatchReceipt> {
    validateBatchShape(taskId, generation, batch)
    const computedBatchDigest = await computeArchiveBatchDigest(
      batch.receipt.previousBatchDigest,
      batch.events,
      batch.seriesLatest,
    )
    if (computedBatchDigest !== batch.receipt.batchDigest) {
      throw new StorageIntegrityError('Archive batch digest does not match its contents')
    }
    const sourceIndexDigest = await computeArchiveSourcePageDigest(batch.events)
    const sourceSeriesDigest = await computeSeriesStateDigest(batch.seriesLatest)
    const seriesCoverage = buildBatchSeriesCoverage(batch)

    return this.sql.begin(async (sql) => {
      const tx = sql as unknown as PostgresClient
      const taskRows = await tx`
        SELECT storage_state, storage_epoch, active_release_generation
        FROM ${tx(TASKS)}
        WHERE id = ${taskId}
        FOR UPDATE
      `
      const task = taskRows[0]
      if (!task) {
        throw new StorageIntegrityError(`Archive task does not exist: ${taskId}`)
      }
      const generationRows = await tx`
        SELECT *
        FROM ${tx(ARCHIVE_GENERATIONS)}
        WHERE task_id = ${taskId} AND generation = ${generation}
        FOR UPDATE
      `
      const generationRow = generationRows[0]
      if (!generationRow) throw new StorageIntegrityError('Archive generation does not exist')

      const archive = rowToArchiveGeneration(generationRow)
      if (
        task['storage_state'] !== 'releasing' ||
        Number(task['storage_epoch']) !== archive.storageEpoch ||
        task['active_release_generation'] !== generation
      ) {
        throw new StorageIntegrityError('Archive batch lost its active task release fence')
      }
      validateSeriesStateBounds(batch.seriesLatest, archive.targetWatermark)
      const existingRows = await tx`
        SELECT *
        FROM ${tx(ARCHIVE_BATCHES)}
        WHERE task_id = ${taskId}
          AND generation = ${generation}
          AND ordinal = ${batch.receipt.ordinal}
      `
      const existing = existingRows[0]
      if (existing) {
        const restored = rowToArchiveBatchReceipt(existing)
        if (
          !archiveBatchReceiptsEqual(restored, batch.receipt) ||
          existing['source_index_digest'] !== sourceIndexDigest ||
          existing['source_series_digest'] !== sourceSeriesDigest ||
          canonicalJson(existing['series_coverage']) !== canonicalJson(seriesCoverage)
        ) {
          throw new StorageIntegrityError('Archive batch replay conflicts with durable receipt')
        }
        return restored
      }
      if (archive.status !== 'open') {
        throw new StorageIntegrityError('Cannot append a batch to a finalized archive generation')
      }

      const receiptRows = await tx`
        SELECT ordinal, current_digest, source_last_index
        FROM ${tx(ARCHIVE_BATCHES)}
        WHERE task_id = ${taskId} AND generation = ${generation}
        ORDER BY ordinal ASC
      `
      const expectedOrdinal = archive.manifest.expectedBatchOrdinals[receiptRows.length]
      if (expectedOrdinal === undefined || expectedOrdinal !== batch.receipt.ordinal) {
        throw new StorageIntegrityError('Archive batches must be uploaded once in manifest order')
      }
      const expectedPrevious =
        receiptRows.length === 0
          ? null
          : (receiptRows[receiptRows.length - 1]?.['current_digest'] as string)
      if (batch.receipt.previousBatchDigest !== expectedPrevious) {
        throw new StorageIntegrityError('Archive batch previous digest breaks the receipt chain')
      }
      const previousLastIndex =
        receiptRows.length === 0
          ? archive.manifest.priorWatermark
          : Number(receiptRows[receiptRows.length - 1]?.['source_last_index'])
      if (batch.receipt.firstIndex === null || batch.receipt.firstIndex <= previousLastIndex) {
        throw new StorageIntegrityError('Archive batch source coverage overlaps or goes backwards')
      }
      for (const event of batch.events) {
        if (
          event.index <= archive.manifest.priorWatermark ||
          event.index > archive.manifest.targetWatermark
        ) {
          throw new StorageIntegrityError('Archive batch event falls outside the sealed watermark')
        }
        if (event.seriesMode === 'latest' || event.seriesMode === 'accumulate') {
          await this.assertCompactEventCompatibleWithClient(tx, event)
          continue
        }
        await this.upsertCanonicalEventWithClient(tx, event)
      }

      await tx`
        INSERT INTO ${tx(ARCHIVE_BATCHES)} (
          task_id, generation, ordinal, previous_digest, current_digest,
          source_first_index, source_last_index, source_index_digest,
          source_series_digest, series_coverage, entry_count, created_at
        ) VALUES (
          ${taskId}, ${generation}, ${batch.receipt.ordinal},
          ${batch.receipt.previousBatchDigest}, ${batch.receipt.batchDigest},
          ${batch.receipt.firstIndex}, ${batch.receipt.lastIndex},
          ${sourceIndexDigest}, ${sourceSeriesDigest},
          ${tx.json(seriesCoverage as never)},
          ${batch.receipt.entryCount}, ${Date.now()}
        )
      `
      await tx`
        UPDATE ${tx(ARCHIVE_GENERATIONS)}
        SET updated_at = ${Date.now()}
        WHERE task_id = ${taskId} AND generation = ${generation}
      `
      return batch.receipt
    })
  }

  async finalizeArchive(
    taskId: string,
    generation: string,
    task: Task,
    seriesLatest: DurableSeriesState[],
  ): Promise<number> {
    if (task.id !== taskId) throw new StorageIntegrityError('Final archive task ID does not match')
    validateSeriesState(taskId, seriesLatest)
    const seriesDigest = await computeSeriesStateDigest(seriesLatest)

    return this.sql.begin(async (sql) => {
      const tx = sql as unknown as PostgresClient
      const taskRows = await tx`
        SELECT *
        FROM ${tx(TASKS)}
        WHERE id = ${taskId}
        FOR UPDATE
      `
      const taskRow = taskRows[0]
      if (!taskRow) throw new StorageIntegrityError(`Archive task does not exist: ${taskId}`)

      const generationRows = await tx`
        SELECT *
        FROM ${tx(ARCHIVE_GENERATIONS)}
        WHERE task_id = ${taskId} AND generation = ${generation}
        FOR UPDATE
      `
      const generationRow = generationRows[0]
      if (!generationRow) throw new StorageIntegrityError('Archive generation does not exist')
      const archive = rowToArchiveGeneration(generationRow)
      validateSeriesStateBounds(seriesLatest, archive.targetWatermark)

      if (archive.status === 'finalized') {
        if (
          Number(taskRow['archive_watermark']) < archive.targetWatermark ||
          seriesDigest !== archive.manifest.seriesStateDigest
        ) {
          throw new StorageIntegrityError('Finalized archive response replay failed verification')
        }
        return archive.targetWatermark
      }
      if (archive.status !== 'open') {
        throw new StorageIntegrityError('Archive generation cannot be finalized')
      }
      if (
        taskRow['storage_state'] !== 'releasing' ||
        Number(taskRow['storage_epoch']) !== archive.storageEpoch ||
        taskRow['active_release_generation'] !== generation
      ) {
        throw new StorageIntegrityError('Archive generation lost its task release fence')
      }

      const receiptRows = await tx`
        SELECT *
        FROM ${tx(ARCHIVE_BATCHES)}
        WHERE task_id = ${taskId} AND generation = ${generation}
        ORDER BY ordinal ASC
      `
      const receipts = receiptRows.map(rowToArchiveBatchReceipt)
      const ordinals = receipts.map((receipt) => receipt.ordinal)
      if (
        canonicalJson(ordinals) !== canonicalJson(archive.manifest.expectedBatchOrdinals)
      ) {
        throw new StorageIntegrityError('Archive generation has missing or unexpected batch ordinals')
      }

      let previousDigest: string | null = null
      let previousLastIndex = archive.manifest.priorWatermark
      let entryCount = 0
      const sourcePageDigests: string[] = []
      const stagedSeries = new Map<string, ArchiveSeriesCoverage>()
      for (let index = 0; index < receipts.length; index++) {
        const receipt = receipts[index]!
        if (receipt.previousBatchDigest !== previousDigest) {
          throw new StorageIntegrityError('Archive generation contains a broken batch digest chain')
        }
        if (
          receipt.entryCount <= 0 ||
          receipt.firstIndex === null ||
          receipt.lastIndex === null ||
          receipt.firstIndex <= previousLastIndex ||
          receipt.lastIndex < receipt.firstIndex ||
          receipt.lastIndex > archive.targetWatermark
        ) {
          throw new StorageIntegrityError('Archive generation contains invalid source coverage')
        }
        const row = receiptRows[index]!
        previousDigest = receipt.batchDigest
        previousLastIndex = receipt.lastIndex
        entryCount += receipt.entryCount
        sourcePageDigests.push(row['source_index_digest'] as string)
        const rowCoverage = row['series_coverage'] as ArchiveSeriesCoverage[]
        for (const coverage of rowCoverage) {
          validateSeriesCoverage(coverage, archive.targetWatermark)
          const previous = stagedSeries.get(coverage.seriesId)
          if (
            previous &&
            (coverage.throughIndex < previous.throughIndex ||
              (coverage.throughIndex === previous.throughIndex &&
                canonicalJson(coverage) !== canonicalJson(previous)) ||
              coverage.mode !== previous.mode)
          ) {
            throw new StorageIntegrityError(
              `Archive generation contains conflicting staged state for series ${coverage.seriesId}`,
            )
          }
          if (!previous || coverage.throughIndex > previous.throughIndex) {
            stagedSeries.set(coverage.seriesId, coverage)
          }
        }
      }
      if (entryCount !== archive.manifest.sourceEntryCount) {
        throw new StorageIntegrityError('Archive generation source entry count does not match manifest')
      }
      if (
        entryCount > 0 &&
        previousLastIndex !== archive.manifest.targetWatermark
      ) {
        throw new StorageIntegrityError('Archive generation does not reach its target watermark')
      }
      if (
        (await computeArchiveSourceDigest(sourcePageDigests)) !== archive.manifest.sourceDigest
      ) {
        throw new StorageIntegrityError('Archive generation source coverage digest does not match')
      }
      if (seriesDigest !== archive.manifest.seriesStateDigest) {
        throw new StorageIntegrityError('Archive generation series state digest does not match')
      }
      const finalSeries = new Map(seriesLatest.map((state) => [state.seriesId, state]))
      for (const [seriesId, coverage] of stagedSeries) {
        const final = finalSeries.get(seriesId)
        if (
          !final ||
          final.mode !== coverage.mode ||
          final.throughIndex < coverage.throughIndex
        ) {
          throw new StorageIntegrityError(
            `Archive final state does not cover compact source series ${seriesId}`,
          )
        }
      }

      const committedSeriesRows = await tx`
        SELECT * FROM ${tx(SERIES_STATE)}
        WHERE task_id = ${taskId}
        FOR UPDATE
      `
      const committedSeries = committedSeriesRows.map(
        (row): DurableSeriesState => ({
          taskId: row['task_id'] as string,
          seriesId: row['series_id'] as string,
          mode: row['mode'] as DurableSeriesState['mode'],
          event: row['event'] as TaskEvent,
          throughIndex: Number(row['through_index']),
        }),
      )
      for (const committed of committedSeries) {
        const final = finalSeries.get(committed.seriesId)
        if (
          !final ||
          final.mode !== committed.mode ||
          final.event.seriesAccField !== committed.event.seriesAccField ||
          final.throughIndex < committed.throughIndex ||
          (final.throughIndex === committed.throughIndex &&
            durableSeriesStateRecord(final) !== durableSeriesStateRecord(committed))
        ) {
          throw new StorageIntegrityError(
            `Archive final state regresses committed series ${committed.seriesId}`,
          )
        }
      }
      const compactEventRows = await tx`
        SELECT * FROM ${tx(EVENTS)}
        WHERE task_id = ${taskId}
          AND series_mode IN ('latest', 'accumulate')
        FOR UPDATE
      `
      for (const row of compactEventRows) {
        const existing = this._rowToEvent(row)
        const seriesId = existing.seriesId
        const final = seriesId ? finalSeries.get(seriesId) : undefined
        if (
          !seriesId ||
          !final ||
          final.mode !== existing.seriesMode ||
          final.event.seriesAccField !== existing.seriesAccField ||
          final.throughIndex < existing.index
        ) {
          throw new StorageIntegrityError(
            `Archive final state omits committed compact series ${seriesId ?? '<missing>'}`,
          )
        }
      }

      await this.saveTaskWithClient(tx, task)
      await tx`DELETE FROM ${tx(SERIES_STATE)} WHERE task_id = ${taskId}`
      for (const state of seriesLatest) {
        await tx`
          INSERT INTO ${tx(SERIES_STATE)} (
            task_id, series_id, mode, event, through_index, updated_at
          ) VALUES (
            ${taskId}, ${state.seriesId}, ${state.mode},
            ${tx.json(state.event as never)}, ${state.throughIndex}, ${Date.now()}
          )
        `
        const committed = committedSeries.find(
          (candidate) => candidate.seriesId === state.seriesId,
        )
        await this.installCanonicalSeriesEventWithClient(tx, state, committed)
      }

      const updateRows = await tx`
        UPDATE ${tx(TASKS)}
        SET archive_watermark = GREATEST(archive_watermark, ${archive.targetWatermark})
        WHERE id = ${taskId}
          AND storage_state = 'releasing'
          AND storage_epoch = ${archive.storageEpoch}
          AND active_release_generation = ${generation}
        RETURNING archive_watermark
      `
      if (updateRows.length !== 1) {
        throw new StorageIntegrityError('Archive task fence changed during finalization')
      }
      await tx`
        UPDATE ${tx(ARCHIVE_GENERATIONS)}
        SET status = 'finalized', finalized_at = ${Date.now()}, updated_at = ${Date.now()}
        WHERE task_id = ${taskId} AND generation = ${generation}
      `
      return Number(updateRows[0]!['archive_watermark'])
    })
  }

  async getArchiveWatermark(taskId: string): Promise<number> {
    const rows = await this.sql`
      SELECT archive_watermark FROM ${this.sql(TASKS)} WHERE id = ${taskId}
    `
    if (!rows[0]) throw new StorageIntegrityError(`Task does not exist: ${taskId}`)
    return Number(rows[0]['archive_watermark'])
  }

  async getLastEventIndex(taskId: string): Promise<number> {
    const rows = await this.sql`
      SELECT GREATEST(
        task.archive_watermark,
        COALESCE((SELECT MAX(event.idx) FROM ${this.sql(EVENTS)} event WHERE event.task_id = task.id), -1),
        COALESCE((SELECT MAX(series.through_index) FROM ${this.sql(SERIES_STATE)} series WHERE series.task_id = task.id), -1)
      ) AS last_index
      FROM ${this.sql(TASKS)} task
      WHERE task.id = ${taskId}
    `
    return rows[0] ? Number(rows[0]['last_index']) : -1
  }

  async getRecentEvents(taskId: string, limit: number): Promise<TaskEvent[]> {
    if (!Number.isSafeInteger(limit) || limit < 0) {
      throw new StorageIntegrityError('Recent event limit must be a non-negative integer')
    }
    if (limit === 0) return []
    const rows = await this.sql`
      SELECT * FROM ${this.sql(EVENTS)}
      WHERE task_id = ${taskId}
      ORDER BY idx DESC
      LIMIT ${limit}
    `
    return rows.reverse().map((row) => this._rowToEvent(row))
  }

  async getDurableSeriesState(taskId: string): Promise<DurableSeriesState[]> {
    const rows = await this.sql`
      SELECT * FROM ${this.sql(SERIES_STATE)}
      WHERE task_id = ${taskId}
      ORDER BY series_id ASC
    `
    return rows.map((row) => ({
      taskId: row['task_id'] as string,
      seriesId: row['series_id'] as string,
      mode: row['mode'] as DurableSeriesState['mode'],
      event: row['event'] as TaskEvent,
      throughIndex: Number(row['through_index']),
    }))
  }

  private async upsertCanonicalEventWithClient(
    sql: PostgresClient,
    event: TaskEvent,
  ): Promise<void> {
    const loadConflicts = () => sql`
      SELECT * FROM ${sql(EVENTS)}
      WHERE id = ${event.id}
         OR (task_id = ${event.taskId} AND idx = ${event.index})
      FOR UPDATE
    `
    const existingRows = await loadConflicts()
    if (existingRows.length > 0) {
      if (
        existingRows.length !== 1 ||
        archiveEventComparable(this._rowToEvent(existingRows[0]!)) !==
          archiveEventComparable(event)
      ) {
        throw new StorageIntegrityError(
          `Archive event identity conflicts at ${event.taskId}:${event.index}`,
        )
      }
      return
    }
    const inserted = await sql`
      INSERT INTO ${sql(EVENTS)} (
        id, task_id, idx, timestamp, type, level, data, series_id, series_mode, series_acc_field
      ) VALUES (
        ${event.id}, ${event.taskId}, ${event.index}, ${event.timestamp},
        ${event.type}, ${event.level},
        ${event.data != null ? sql.json(event.data as never) : null},
        ${event.seriesId ?? null}, ${event.seriesMode ?? null},
        ${event.seriesAccField ?? null}
      )
      ON CONFLICT DO NOTHING
      RETURNING id
    `
    if (inserted.length === 1) return

    const racedRows = await loadConflicts()
    if (
      racedRows.length !== 1 ||
      archiveEventComparable(this._rowToEvent(racedRows[0]!)) !==
        archiveEventComparable(event)
    ) {
      throw new StorageIntegrityError(
        `Archive event identity conflicts at ${event.taskId}:${event.index}`,
      )
    }
  }

  private async assertCompactEventCompatibleWithClient(
    sql: PostgresClient,
    event: TaskEvent,
  ): Promise<void> {
    const rows = await sql`
      SELECT * FROM ${sql(EVENTS)}
      WHERE id = ${event.id}
         OR (task_id = ${event.taskId} AND idx = ${event.index})
      FOR UPDATE
    `
    if (rows.length === 0) return
    if (rows.length !== 1) {
      throw new StorageIntegrityError(
        `Archive compact event identity conflicts at ${event.taskId}:${event.index}`,
      )
    }
    const existing = this._rowToEvent(rows[0]!)
    const fieldMatches =
      event.seriesMode === 'accumulate'
        ? (existing.seriesAccField ?? 'delta') ===
          (event.seriesAccField ?? 'delta')
        : existing.seriesAccField === event.seriesAccField
    if (
      existing.taskId !== event.taskId ||
      existing.seriesId !== event.seriesId ||
      existing.seriesMode !== event.seriesMode ||
      !fieldMatches
    ) {
      throw new StorageIntegrityError(
        `Archive compact event identity conflicts at ${event.taskId}:${event.index}`,
      )
    }
  }

  private async installCanonicalSeriesEventWithClient(
    sql: PostgresClient,
    state: DurableSeriesState,
    committed: DurableSeriesState | undefined,
  ): Promise<void> {
    const event = state.event
    const rows = await sql`
      SELECT * FROM ${sql(EVENTS)}
      WHERE (task_id = ${event.taskId}
             AND series_id = ${state.seriesId}
             AND series_mode IN ('latest', 'accumulate'))
         OR id = ${event.id}
         OR (task_id = ${event.taskId} AND idx = ${event.index})
      FOR UPDATE
    `
    const existing = rows.map((row) => this._rowToEvent(row))
    const seriesRows = existing.filter(
      (candidate) =>
        candidate.taskId === event.taskId &&
        candidate.seriesId === state.seriesId,
    )
    if (
      seriesRows.some(
        (candidate) =>
          candidate.seriesMode !== state.mode ||
          candidate.seriesAccField !== event.seriesAccField,
      )
    ) {
      throw new StorageIntegrityError(
        `Archive series semantics conflict for ${state.seriesId}`,
      )
    }

    const identityRows = existing.filter(
      (candidate) =>
        candidate.id === event.id ||
        (candidate.taskId === event.taskId && candidate.index === event.index),
    )
    if (identityRows.length > 1) {
      throw new StorageIntegrityError(
        `Archive event identity conflicts at ${event.taskId}:${event.index}`,
      )
    }
    const identity = identityRows[0]
    if (identity) {
      if (archiveEventComparable(identity) !== archiveEventComparable(event)) {
        const canAdvanceAccumulation =
          state.mode === 'accumulate' &&
          identity.taskId === event.taskId &&
          identity.id === event.id &&
          identity.index === event.index &&
          identity.seriesId === state.seriesId &&
          identity.seriesMode === 'accumulate' &&
          identity.seriesAccField === event.seriesAccField &&
          state.throughIndex >
            (committed?.throughIndex ?? identity.index)
        if (!canAdvanceAccumulation) {
          throw new StorageIntegrityError(
            `Archive event identity conflicts at ${event.taskId}:${event.index}`,
          )
        }
        await this.updateStoredSeriesEventWithClient(sql, identity, event)
      }
    } else {
      await this.upsertCanonicalEventWithClient(sql, event)
    }

    await sql`
      DELETE FROM ${sql(EVENTS)}
      WHERE task_id = ${event.taskId}
        AND series_id = ${state.seriesId}
        AND series_mode IN ('latest', 'accumulate')
        AND id <> ${event.id}
    `
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    return this.observed(async () => {
      const t = EVENTS
      const since = opts?.since

      let rows: postgres.RowList<postgres.Row[]>
      if (since?.index !== undefined) {
        rows = await this.sql`
          SELECT * FROM ${this.sql(t)}
          WHERE task_id = ${taskId} AND idx > ${since.index}
          ORDER BY idx ASC
          ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
        `
      } else if (since?.timestamp !== undefined) {
        rows = await this.sql`
          SELECT * FROM ${this.sql(t)}
          WHERE task_id = ${taskId} AND timestamp > ${since.timestamp}
          ORDER BY idx ASC
          ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
        `
      } else if (since?.id) {
        const anchor = await this.sql`
          SELECT idx FROM ${this.sql(t)} WHERE id = ${since.id}
        `
        const anchorIdx = (anchor[0]?.['idx'] as number | undefined) ?? -1
        rows = await this.sql`
          SELECT * FROM ${this.sql(t)}
          WHERE task_id = ${taskId} AND idx > ${anchorIdx}
          ORDER BY idx ASC
          ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
        `
      } else {
        rows = await this.sql`
          SELECT * FROM ${this.sql(t)}
          WHERE task_id = ${taskId}
          ORDER BY idx ASC
          ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
        `
      }

      return rows.map((r) => this._rowToEvent(r))
    })
  }

  async saveWorkerEvent(event: WorkerAuditEvent): Promise<void> {
    return this.observed(async () => {
      const t = WORKER_EVENTS
      await this.sql`
        INSERT INTO ${this.sql(t)} (
          id, worker_id, timestamp, action, data
        ) VALUES (
          ${event.id}, ${event.workerId}, ${event.timestamp},
          ${event.action},
          ${event.data ? this.sql.json(event.data as never) : null}
        )
        ON CONFLICT (id) DO NOTHING
      `
    })
  }

  async getWorkerEvents(workerId: string, opts?: EventQueryOptions): Promise<WorkerAuditEvent[]> {
    return this.observed(async () => {
      const t = WORKER_EVENTS
      const since = opts?.since

      let rows: postgres.RowList<postgres.Row[]>
      if (since?.timestamp !== undefined) {
        rows = await this.sql`
          SELECT * FROM ${this.sql(t)}
          WHERE worker_id = ${workerId} AND timestamp > ${since.timestamp}
          ORDER BY timestamp ASC
          ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
        `
      } else if (since?.id) {
        const anchor = await this.sql`
          SELECT timestamp FROM ${this.sql(t)} WHERE id = ${since.id}
        `
        const anchorTs = (anchor[0]?.['timestamp'] as number | undefined) ?? 0
        rows = await this.sql`
          SELECT * FROM ${this.sql(t)}
          WHERE worker_id = ${workerId} AND timestamp > ${anchorTs}
          ORDER BY timestamp ASC
          ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
        `
      } else {
        rows = await this.sql`
          SELECT * FROM ${this.sql(t)}
          WHERE worker_id = ${workerId}
          ORDER BY timestamp ASC
          ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
        `
      }

      return rows.map((r) => this._rowToWorkerEvent(r))
    })
  }

  private _rowToTask(row: postgres.Row): Task {
    // Build using mutable assignment to satisfy exactOptionalPropertyTypes
    // Note: PostgreSQL BIGINT comes back as string from postgres.js, so we use Number() for numeric columns
    const task: Task = {
      id: row['id'] as string,
      status: row['status'] as Task['status'],
      createdAt: Number(row['created_at']),
      updatedAt: Number(row['updated_at']),
    }
    if (row['type'] != null) task.type = row['type'] as string
    if (row['params'] != null) task.params = row['params'] as Record<string, unknown>
    if (row['result'] != null) task.result = row['result'] as Record<string, unknown>
    if (row['error'] != null) task.error = row['error'] as TaskError
    if (row['metadata'] != null) task.metadata = row['metadata'] as Record<string, unknown>
    if (row['auth_config'] != null) task.authConfig = row['auth_config'] as TaskAuthConfig
    if (row['webhooks'] != null) task.webhooks = row['webhooks'] as WebhookConfig[]
    if (row['cleanup'] != null) task.cleanup = row['cleanup'] as { rules: CleanupRule[] }
    if (row['completed_at'] != null) task.completedAt = Number(row['completed_at'])
    if (row['ttl'] != null) task.ttl = Number(row['ttl'])
    if (row['tags'] != null) task.tags = row['tags'] as string[]
    if (row['assign_mode'] != null) task.assignMode = row['assign_mode'] as AssignMode
    if (row['cost'] != null) task.cost = Number(row['cost'])
    if (row['assigned_worker'] != null) task.assignedWorker = row['assigned_worker'] as string
    if (row['disconnect_policy'] != null) task.disconnectPolicy = row['disconnect_policy'] as DisconnectPolicy
    return task
  }

  private _rowToEvent(row: postgres.Row): TaskEvent {
    // Build using mutable assignment to satisfy exactOptionalPropertyTypes
    const event: TaskEvent = {
      id: row['id'] as string,
      taskId: row['task_id'] as string,
      index: Number(row['idx']),
      timestamp: Number(row['timestamp']),
      type: row['type'] as string,
      level: row['level'] as TaskEvent['level'],
      data: (row['data'] as unknown) ?? null,
    }
    if (row['series_id'] != null) event.seriesId = row['series_id'] as string
    if (row['series_mode'] != null) event.seriesMode = row['series_mode'] as SeriesMode
    if (row['series_acc_field'] != null) event.seriesAccField = row['series_acc_field'] as string
    return event
  }

  private _rowToWorkerEvent(row: postgres.Row): WorkerAuditEvent {
    const event: WorkerAuditEvent = {
      id: row['id'] as string,
      workerId: row['worker_id'] as string,
      timestamp: Number(row['timestamp']),
      action: row['action'] as WorkerAuditEvent['action'],
    }
    if (row['data'] != null) event.data = row['data'] as Record<string, unknown>
    return event
  }
}

function validateArchiveManifest(manifest: ArchiveSourceManifest): void {
  if (
    !Number.isSafeInteger(manifest.priorWatermark) ||
    !Number.isSafeInteger(manifest.targetWatermark) ||
    manifest.targetWatermark < manifest.priorWatermark ||
    !Number.isSafeInteger(manifest.sourceEntryCount) ||
    manifest.sourceEntryCount < 0
  ) {
    throw new StorageIntegrityError('Archive manifest contains invalid source bounds')
  }
  if (
    (manifest.sourceEntryCount === 0 &&
      manifest.targetWatermark !== manifest.priorWatermark) ||
    (manifest.sourceEntryCount > 0 &&
      manifest.targetWatermark === manifest.priorWatermark) ||
    (manifest.sourceEntryCount === 0 && manifest.expectedBatchOrdinals.length !== 0) ||
    (manifest.sourceEntryCount > 0 &&
      (manifest.expectedBatchOrdinals.length === 0 ||
        manifest.expectedBatchOrdinals.length > manifest.sourceEntryCount))
  ) {
    throw new StorageIntegrityError('Archive manifest batch count is inconsistent with its source')
  }
  for (let index = 0; index < manifest.expectedBatchOrdinals.length; index++) {
    if (manifest.expectedBatchOrdinals[index] !== index) {
      throw new StorageIntegrityError('Archive manifest batch ordinals must be contiguous from zero')
    }
  }
  if (
    !/^[a-f0-9]{64}$/.test(manifest.sourceDigest) ||
    !/^[a-f0-9]{64}$/.test(manifest.seriesStateDigest)
  ) {
    throw new StorageIntegrityError('Archive manifest digests must be lowercase SHA-256')
  }
}

function validateStorageMetadataCas(update: TaskStorageMetadataCas): void {
  const next = update.next
  if (next.taskId !== update.taskId) {
    throw new StorageIntegrityError('Storage metadata task ID does not match CAS target')
  }
  if (
    !Number.isSafeInteger(update.expectedStorageEpoch) ||
    !Number.isSafeInteger(next.storageEpoch) ||
    next.storageEpoch < 1 ||
    next.storageEpoch < update.expectedStorageEpoch ||
    !Number.isSafeInteger(next.archiveWatermark) ||
    next.archiveWatermark < -1 ||
    !Number.isSafeInteger(next.taskVersion) ||
    next.taskVersion < 0
  ) {
    throw new StorageIntegrityError('Storage metadata CAS would violate a monotonic counter')
  }
  if (
    (next.storageState === 'releasing' && next.activeReleaseGeneration === null) ||
    (next.storageState === 'hot' && next.activeReleaseGeneration !== null)
  ) {
    throw new StorageIntegrityError('Storage metadata release generation is inconsistent')
  }
  for (const timestamp of [
    next.lastEventAt,
    next.coldAt,
    next.executionDeadlineAt,
  ]) {
    if (timestamp !== null && !Number.isSafeInteger(timestamp)) {
      throw new StorageIntegrityError('Storage metadata timestamps must be safe integers')
    }
  }
}

function validateStorageReleaseRequest(request: StorageReleaseRequest): void {
  if (
    request.taskId.length === 0 ||
    !Number.isSafeInteger(request.requestedAt) ||
    request.requestedAt < 0 ||
    !Number.isSafeInteger(request.expectedLastEventIndex) ||
    request.expectedLastEventIndex < -1 ||
    !Number.isSafeInteger(request.inactiveSince) ||
    request.inactiveSince < 0
  ) {
    throw new StorageIntegrityError('Storage release request is invalid')
  }
}

function validateBatchShape(taskId: string, generation: string, batch: ArchiveBatch): void {
  const receipt = batch.receipt
  if (receipt.taskId !== taskId || receipt.generation !== generation) {
    throw new StorageIntegrityError('Archive batch identity does not match its generation')
  }
  if (
    !Number.isSafeInteger(receipt.ordinal) ||
    receipt.ordinal < 0 ||
    receipt.ordinal > POSTGRES_INTEGER_MAX ||
    receipt.entryCount !== batch.events.length ||
    receipt.entryCount > POSTGRES_INTEGER_MAX ||
    batch.events.length === 0
  ) {
    throw new StorageIntegrityError('Archive batch receipt count or ordinal is invalid')
  }
  const firstIndex = batch.events[0]?.index ?? null
  const lastIndex = batch.events[batch.events.length - 1]?.index ?? null
  if (receipt.firstIndex !== firstIndex || receipt.lastIndex !== lastIndex) {
    throw new StorageIntegrityError('Archive batch receipt coverage does not match its events')
  }
  let previousIndex = -1
  const eventIds = new Set<string>()
  for (const event of batch.events) {
    if (
      event.taskId !== taskId ||
      event.id.length === 0 ||
      !Number.isSafeInteger(event.index) ||
      event.index < 0 ||
      event.index > POSTGRES_INTEGER_MAX ||
      !Number.isSafeInteger(event.timestamp) ||
      event.index <= previousIndex ||
      eventIds.has(event.id) ||
      event.seriesSnapshot !== undefined ||
      event._accumulatedData !== undefined
    ) {
      throw new StorageIntegrityError('Archive batch events must have unique, increasing identities')
    }
    assertCanonicalJson(event.data)
    previousIndex = event.index
    eventIds.add(event.id)
  }
  validateSeriesState(taskId, batch.seriesLatest)
}

function validateSeriesState(taskId: string, states: readonly DurableSeriesState[]): void {
  const seriesIds = new Set<string>()
  for (const state of states) {
    if (
      state.taskId !== taskId ||
      state.event.taskId !== taskId ||
      state.event.seriesId !== state.seriesId ||
      state.event.seriesMode !== state.mode ||
      state.throughIndex < state.event.index ||
      !Number.isSafeInteger(state.throughIndex) ||
      state.throughIndex < 0 ||
      state.throughIndex > POSTGRES_INTEGER_MAX ||
      state.seriesId.length === 0 ||
      !Number.isSafeInteger(state.event.index) ||
      state.event.index < 0 ||
      state.event.index > POSTGRES_INTEGER_MAX ||
      !Number.isSafeInteger(state.event.timestamp) ||
      state.event.seriesSnapshot !== undefined ||
      state.event._accumulatedData !== undefined ||
      seriesIds.has(state.seriesId)
    ) {
      throw new StorageIntegrityError('Archive durable series state is inconsistent')
    }
    assertCanonicalJson(state.event.data)
    seriesIds.add(state.seriesId)
  }
}

function validateSeriesStateBounds(
  states: readonly DurableSeriesState[],
  targetWatermark: number,
): void {
  if (
    states.some(
      (state) =>
        state.event.index > targetWatermark || state.throughIndex > targetWatermark,
    )
  ) {
    throw new StorageIntegrityError(
      'Archive durable series state exceeds the sealed watermark',
    )
  }
}

function buildBatchSeriesCoverage(batch: ArchiveBatch): ArchiveSeriesCoverage[] {
  const compactEvents = batch.events.filter(
    (event) => event.seriesMode === 'latest' || event.seriesMode === 'accumulate',
  )
  const coverage = new Map<string, ArchiveSeriesCoverage>()
  for (const event of compactEvents) {
    const mode = event.seriesMode
    if (mode !== 'latest' && mode !== 'accumulate') continue
    if (!event.seriesId) {
      throw new StorageIntegrityError('Archive compact event is missing its series ID')
    }
    const previous = coverage.get(event.seriesId)
    if (previous && previous.mode !== mode) {
      throw new StorageIntegrityError(
        `Archive compact source changes mode for series ${event.seriesId}`,
      )
    }
    coverage.set(event.seriesId, {
      seriesId: event.seriesId,
      mode,
      throughIndex: Math.max(previous?.throughIndex ?? -1, event.index),
    })
  }
  return [...coverage.values()].sort((left, right) =>
    compareUtf8Strings(left.seriesId, right.seriesId),
  )
}

function validateSeriesCoverage(
  coverage: ArchiveSeriesCoverage,
  targetWatermark: number,
): void {
  if (
    coverage.seriesId.length === 0 ||
    (coverage.mode !== 'latest' && coverage.mode !== 'accumulate') ||
    !Number.isSafeInteger(coverage.throughIndex) ||
    coverage.throughIndex < 0 ||
    coverage.throughIndex > targetWatermark
  ) {
    throw new StorageIntegrityError('Archive compact series coverage is invalid')
  }
}

function compareUtf8Strings(left: string, right: string): number {
  const encoder = new TextEncoder()
  const leftBytes = encoder.encode(left)
  const rightBytes = encoder.encode(right)
  const length = Math.min(leftBytes.length, rightBytes.length)
  for (let index = 0; index < length; index++) {
    const difference = leftBytes[index]! - rightBytes[index]!
    if (difference !== 0) return difference
  }
  return leftBytes.length - rightBytes.length
}

function assertCanonicalJson(value: unknown): void {
  try {
    canonicalJson(value)
  } catch {
    throw new StorageIntegrityError('Archive event data must be canonical JSON')
  }
}

function rowToStorageMetadata(row: postgres.Row): TaskStorageMetadata {
  return {
    taskId: row['id'] as string,
    storageState: row['storage_state'] as TaskStorageMetadata['storageState'],
    storageEpoch: Number(row['storage_epoch']),
    activeReleaseGeneration: (row['active_release_generation'] as string | null) ?? null,
    archiveWatermark: Number(row['archive_watermark']),
    lastEventAt: row['last_event_at'] == null ? null : Number(row['last_event_at']),
    coldAt: row['cold_at'] == null ? null : Number(row['cold_at']),
    executionDeadlineAt:
      row['execution_deadline_at'] == null ? null : Number(row['execution_deadline_at']),
    taskVersion: Number(row['task_version']),
  }
}

function rowToArchiveGeneration(row: postgres.Row): ArchiveGeneration {
  const storedStatus = row['status'] as string
  return {
    taskId: row['task_id'] as string,
    generation: row['generation'] as string,
    storageEpoch: Number(row['storage_epoch']),
    targetWatermark: Number(row['target_watermark']),
    manifest: row['manifest'] as ArchiveSourceManifest,
    status:
      storedStatus === 'uploading'
        ? 'open'
        : storedStatus === 'finalized'
          ? 'finalized'
          : 'aborted',
    createdAt: Number(row['created_at']),
    updatedAt: Number(row['updated_at']),
  }
}

function rowToArchiveBatchReceipt(row: postgres.Row): ArchiveBatchReceipt {
  return {
    taskId: row['task_id'] as string,
    generation: row['generation'] as string,
    ordinal: Number(row['ordinal']),
    previousBatchDigest: (row['previous_digest'] as string | null) ?? null,
    batchDigest: row['current_digest'] as string,
    entryCount: Number(row['entry_count']),
    firstIndex: row['source_first_index'] == null ? null : Number(row['source_first_index']),
    lastIndex: row['source_last_index'] == null ? null : Number(row['source_last_index']),
  }
}

function archiveBatchReceiptsEqual(
  left: ArchiveBatchReceipt,
  right: ArchiveBatchReceipt,
): boolean {
  return canonicalJson(left) === canonicalJson(right)
}

function archiveEventComparable(event: TaskEvent): string {
  return archiveEventRecord(event)
}
