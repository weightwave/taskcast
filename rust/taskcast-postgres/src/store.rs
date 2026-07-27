use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use crate::classify_postgres_connectivity;
use taskcast_core::archive::{
    archive_event_record, compute_archive_batch_digest, compute_archive_source_digest,
    compute_archive_source_page_digest, compute_series_state_digest, durable_series_state_record,
};
use taskcast_core::types::{
    ArchiveBatch, ArchiveBatchReceipt, ArchiveGeneration, ArchiveGenerationStatus,
    ArchiveSourceManifest, AssignMode, CleanupConfig, DisconnectPolicy, DurableSeriesState,
    EventQueryOptions, Level, LongTermStore, SeriesMode, StorageFenceConflictError,
    StorageIntegrityError, StorageReleaseRequest, StorageState, Task, TaskAuthConfig, TaskError,
    TaskEvent, TaskStatus, TaskStorageMetadata, TaskStorageMetadataCas, TerminalProjection,
    TtlClaim, WebhookConfig, WorkerAssignment, WorkerAuditAction, WorkerAuditEvent,
};
use taskcast_core::{
    BoxError, DependencyName, DependencyObservation, DependencyObservationState,
    DependencyObserver, DependencyUnavailableError,
};

const TASKS: &str = "taskcast_tasks";
const EVENTS: &str = "taskcast_events";
const WORKER_EVENTS: &str = "taskcast_worker_events";
const ARCHIVE_GENERATIONS: &str = "taskcast_archive_generations";
const ARCHIVE_BATCHES: &str = "taskcast_archive_batches";
const SERIES_STATE: &str = "taskcast_series_state";
const DURABLE_ASSIGNMENTS: &str = "taskcast_durable_assignments";
const TERMINAL_OUTBOX: &str = "taskcast_terminal_outbox";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveSeriesCoverage {
    series_id: String,
    mode: SeriesMode,
    through_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalProjectionPayload {
    task: Task,
    event: TaskEvent,
    assignment: Option<WorkerAssignment>,
}

/// PostgreSQL-backed long-term store for tasks and events.
///
/// Uses `sqlx::PgPool` for connection pooling and implements the
/// `LongTermStore` trait from `taskcast-core`.
pub struct PostgresLongTermStore {
    pool: PgPool,
    observer: Option<Arc<dyn DependencyObserver>>,
}

impl PostgresLongTermStore {
    /// Create a new store with the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            observer: None,
        }
    }

    pub fn new_observed(pool: PgPool, observer: Arc<dyn DependencyObserver>) -> Self {
        Self {
            pool,
            observer: Some(observer),
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn observed<T, Operation, OperationFuture>(
        &self,
        operation: Operation,
    ) -> Result<T, BoxError>
    where
        Operation: FnOnce() -> OperationFuture,
        OperationFuture: Future<Output = Result<T, BoxError>>,
    {
        match operation().await {
            Ok(value) => {
                self.observe(DependencyObservationState::Healthy, None);
                Ok(value)
            }
            Err(error) => {
                let Some(kind) = classify_postgres_connectivity(error.as_ref()) else {
                    return Err(error);
                };
                self.observe(DependencyObservationState::Unhealthy, Some(kind));
                let unavailable = match error.downcast::<sqlx::Error>() {
                    Ok(source) => {
                        DependencyUnavailableError::new(DependencyName::Postgres, kind, *source)
                    }
                    Err(source) => DependencyUnavailableError::new(
                        DependencyName::Postgres,
                        kind,
                        BoxedSource(source),
                    ),
                };
                Err(Box::new(unavailable))
            }
        }
    }

    fn observe(
        &self,
        state: DependencyObservationState,
        error_kind: Option<taskcast_core::DependencyErrorKind>,
    ) {
        let Some(observer) = &self.observer else {
            return;
        };
        observer.observe(DependencyObservation {
            dependency: DependencyName::Postgres,
            state,
            error_kind,
            attempt: None,
            next_retry_ms: None,
        });
    }

    /// Run migrations to create/update tables and indexes.
    ///
    /// Uses sqlx's built-in migration runner with `.sql` files from the shared
    /// `migrations/postgres/` directory at the repo root (embedded at compile time).
    /// Tracks applied migrations in the `_sqlx_migrations` table.
    pub async fn migrate(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::migrate!("../../migrations/postgres")
            .run(&self.pool)
            .await?;
        Ok(())
    }

    /// Convert a database row into a `Task`.
    fn row_to_task(row: &PgRow) -> Task {
        let status_str: String = row.get("status");
        let status: TaskStatus =
            serde_json::from_value(JsonValue::String(status_str)).unwrap_or(TaskStatus::Pending);

        let created_at_i64: i64 = row.get("created_at");
        let updated_at_i64: i64 = row.get("updated_at");
        let completed_at_i64: Option<i64> = row.get("completed_at");
        let ttl_i32: Option<i32> = row.get("ttl");

        let params: Option<JsonValue> = row.get("params");
        let result: Option<JsonValue> = row.get("result");
        let error: Option<JsonValue> = row.get("error");
        let metadata: Option<JsonValue> = row.get("metadata");
        let auth_config: Option<JsonValue> = row.get("auth_config");
        let webhooks: Option<JsonValue> = row.get("webhooks");
        let cleanup: Option<JsonValue> = row.get("cleanup");

        let tags: Option<JsonValue> = row.get("tags");
        let assign_mode_str: Option<String> = row.get("assign_mode");
        let cost_i32: Option<i32> = row.get("cost");
        let assigned_worker: Option<String> = row.get("assigned_worker");
        let disconnect_policy_str: Option<String> = row.get("disconnect_policy");

        let assign_mode: Option<AssignMode> =
            assign_mode_str.and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());
        let disconnect_policy: Option<DisconnectPolicy> =
            disconnect_policy_str.and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());

        Task {
            id: row.get("id"),
            r#type: row.get("type"),
            status,
            params: params.and_then(|v| serde_json::from_value(v).ok()),
            result: result.and_then(|v| serde_json::from_value(v).ok()),
            error: error.and_then(|v| serde_json::from_value::<TaskError>(v).ok()),
            metadata: metadata.and_then(|v| serde_json::from_value(v).ok()),
            auth_config: auth_config.and_then(|v| serde_json::from_value::<TaskAuthConfig>(v).ok()),
            webhooks: webhooks.and_then(|v| serde_json::from_value::<Vec<WebhookConfig>>(v).ok()),
            cleanup: cleanup.and_then(|v| serde_json::from_value::<CleanupConfig>(v).ok()),
            created_at: created_at_i64 as f64,
            updated_at: updated_at_i64 as f64,
            completed_at: completed_at_i64.map(|v| v as f64),
            ttl: ttl_i32.map(|v| v as u64),
            tags: tags.and_then(|v| serde_json::from_value::<Vec<String>>(v).ok()),
            assign_mode,
            cost: cost_i32.map(|v| v as u32),
            assigned_worker,
            disconnect_policy,
            reason: None,
            resume_at: None,
            blocked_request: None,
        }
    }

    /// Convert a database row into a `WorkerAuditEvent`.
    fn row_to_worker_event(row: &PgRow) -> WorkerAuditEvent {
        let action_str: String = row.get("action");
        let action: WorkerAuditAction = serde_json::from_value(JsonValue::String(action_str))
            .unwrap_or(WorkerAuditAction::Connected);

        let timestamp_i64: i64 = row.get("timestamp");
        let data: Option<JsonValue> = row.get("data");

        WorkerAuditEvent {
            id: row.get("id"),
            worker_id: row.get("worker_id"),
            timestamp: timestamp_i64 as f64,
            action,
            data: data.and_then(|v| serde_json::from_value(v).ok()),
        }
    }

    /// Convert a database row into a `TaskEvent`.
    fn row_to_event(row: &PgRow) -> TaskEvent {
        let level_str: String = row.get("level");
        let level: Level =
            serde_json::from_value(JsonValue::String(level_str)).unwrap_or(Level::Info);

        let idx: i32 = row.get("idx");
        let timestamp_i64: i64 = row.get("timestamp");
        let data: Option<JsonValue> = row.get("data");

        let series_mode_str: Option<String> = row.get("series_mode");
        let series_mode: Option<SeriesMode> =
            series_mode_str.and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());

        TaskEvent {
            id: row.get("id"),
            task_id: row.get("task_id"),
            index: idx as u64,
            timestamp: timestamp_i64 as f64,
            r#type: row.get("type"),
            level,
            data: data.unwrap_or(JsonValue::Null),
            series_id: row.get("series_id"),
            series_mode,
            series_acc_field: row.get("series_acc_field"),
            series_snapshot: None,
            _accumulated_data: None,
        }
    }
}

struct BoxedSource(BoxError);

impl fmt::Debug for BoxedSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("BoxedSource").field(&self.0).finish()
    }
}

impl fmt::Display for BoxedSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for BoxedSource {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
impl LongTermStore for PostgresLongTermStore {
    fn supports_hot_cold_release(&self) -> bool {
        true
    }

    fn supports_durable_ttl(&self) -> bool {
        true
    }

    fn supports_task_creation_claims(&self) -> bool {
        true
    }

    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
            let params_json: Option<JsonValue> = task
                .params
                .as_ref()
                .map(|p| serde_json::to_value(p).unwrap_or(JsonValue::Null));
            let result_json: Option<JsonValue> = task
                .result
                .as_ref()
                .map(|r| serde_json::to_value(r).unwrap_or(JsonValue::Null));
            let error_json: Option<JsonValue> = task
                .error
                .as_ref()
                .map(|e| serde_json::to_value(e).unwrap_or(JsonValue::Null));
            let metadata_json: Option<JsonValue> = task
                .metadata
                .as_ref()
                .map(|m| serde_json::to_value(m).unwrap_or(JsonValue::Null));
            let auth_config_json: Option<JsonValue> = task
                .auth_config
                .as_ref()
                .map(|a| serde_json::to_value(a).unwrap_or(JsonValue::Null));
            let webhooks_json: Option<JsonValue> = task
                .webhooks
                .as_ref()
                .map(|w| serde_json::to_value(w).unwrap_or(JsonValue::Null));
            let cleanup_json: Option<JsonValue> = task
                .cleanup
                .as_ref()
                .map(|c| serde_json::to_value(c).unwrap_or(JsonValue::Null));

            let created_at = task.created_at as i64;
            let updated_at = task.updated_at as i64;
            let completed_at = task.completed_at.map(|v| v as i64);
            let ttl = task.ttl.map(|v| v as i32);

            let tags_json: Option<JsonValue> = task
                .tags
                .as_ref()
                .map(|t| serde_json::to_value(t).unwrap_or(JsonValue::Null));
            let assign_mode_str: Option<String> = task.assign_mode.as_ref().map(|m| {
                serde_json::to_value(m)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_default()
            });
            let cost_i32: Option<i32> = task.cost.map(|c| c as i32);
            let disconnect_policy_str: Option<String> = task.disconnect_policy.as_ref().map(|d| {
                serde_json::to_value(d)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_default()
            });

            let sql = format!(
                r#"
            INSERT INTO {TASKS} (
                id, type, status, params, result, error, metadata,
                auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
                tags, assign_mode, cost, assigned_worker, disconnect_policy,
                execution_deadline_at, task_version
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19,
                CASE
                    WHEN $20
                    THEN FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
                        + $21 * 1000
                    ELSE NULL
                END,
                0
            )
            ON CONFLICT (id) DO UPDATE SET
                status = EXCLUDED.status,
                result = EXCLUDED.result,
                error = EXCLUDED.error,
                metadata = EXCLUDED.metadata,
                updated_at = EXCLUDED.updated_at,
                completed_at = EXCLUDED.completed_at,
                ttl = EXCLUDED.ttl,
                tags = EXCLUDED.tags,
                assign_mode = EXCLUDED.assign_mode,
                cost = EXCLUDED.cost,
                assigned_worker = EXCLUDED.assigned_worker,
                disconnect_policy = EXCLUDED.disconnect_policy,
                execution_deadline_at = CASE
                    WHEN EXCLUDED.execution_deadline_at IS NULL THEN NULL
                    WHEN {TASKS}.execution_deadline_at IS NULL
                        OR {TASKS}.status = 'paused'
                        OR {TASKS}.ttl IS DISTINCT FROM EXCLUDED.ttl
                    THEN EXCLUDED.execution_deadline_at
                    ELSE {TASKS}.execution_deadline_at
                END,
                task_version = {TASKS}.task_version + 1,
                ttl_claim_token = NULL,
                ttl_claim_until = NULL
            WHERE {TASKS}.status NOT IN ('completed', 'failed', 'timeout', 'cancelled')
               OR {TASKS}.status = EXCLUDED.status
            RETURNING id
            "#
            );

            let status_str = serde_json::to_value(&task.status)
                .map(|v| v.as_str().unwrap_or("pending").to_string())?;

            let saved = sqlx::query(&sql)
                .bind(&task.id)
                .bind(&task.r#type)
                .bind(&status_str)
                .bind(&params_json)
                .bind(&result_json)
                .bind(&error_json)
                .bind(&metadata_json)
                .bind(&auth_config_json)
                .bind(&webhooks_json)
                .bind(&cleanup_json)
                .bind(created_at)
                .bind(updated_at)
                .bind(completed_at)
                .bind(ttl)
                .bind(&tags_json)
                .bind(&assign_mode_str)
                .bind(cost_i32)
                .bind(&task.assigned_worker)
                .bind(&disconnect_policy_str)
                .bind(has_execution_deadline(&task))
                .bind(task.ttl.unwrap_or(0) as i64)
                .fetch_optional(&self.pool)
                .await?;
            if saved.is_none() {
                return Err(Box::new(StorageFenceConflictError::new(format!(
                    "Durable terminal task cannot be overwritten: {}",
                    task.id
                ))) as BoxError);
            }

            Ok(())
        })
        .await
    }

    async fn create_task_if_absent(
        &self,
        task: Task,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let params_json = task.params.as_ref().map(serde_json::to_value).transpose()?;
        let result_json = task.result.as_ref().map(serde_json::to_value).transpose()?;
        let error_json = task.error.as_ref().map(serde_json::to_value).transpose()?;
        let metadata_json = task
            .metadata
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let auth_config_json = task
            .auth_config
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let webhooks_json = task
            .webhooks
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let cleanup_json = task
            .cleanup
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let tags_json = task.tags.as_ref().map(serde_json::to_value).transpose()?;
        let assign_mode = task
            .assign_mode
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .and_then(|value| value.as_str().map(str::to_string));
        let disconnect_policy = task
            .disconnect_policy
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .and_then(|value| value.as_str().map(str::to_string));
        let status = serde_json::to_value(&task.status)?
            .as_str()
            .unwrap_or("pending")
            .to_string();
        let sql = format!(
            r#"
            INSERT INTO {TASKS} (
                id, type, status, params, result, error, metadata,
                auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
                tags, assign_mode, cost, assigned_worker, disconnect_policy,
                execution_deadline_at, task_version
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19,
                CASE
                    WHEN $20
                    THEN FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
                        + $21 * 1000
                    ELSE NULL
                END,
                0
            )
            ON CONFLICT (id) DO NOTHING
            "#
        );
        let result = sqlx::query(&sql)
            .bind(&task.id)
            .bind(&task.r#type)
            .bind(status)
            .bind(params_json)
            .bind(result_json)
            .bind(error_json)
            .bind(metadata_json)
            .bind(auth_config_json)
            .bind(webhooks_json)
            .bind(cleanup_json)
            .bind(task.created_at as i64)
            .bind(task.updated_at as i64)
            .bind(task.completed_at.map(|value| value as i64))
            .bind(task.ttl.map(|value| value as i32))
            .bind(tags_json)
            .bind(assign_mode)
            .bind(task.cost.map(|value| value as i32))
            .bind(&task.assigned_worker)
            .bind(disconnect_policy)
            .bind(has_execution_deadline(&task))
            .bind(task.ttl.unwrap_or(0) as i64)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn claim_task_creation(
        &self,
        task: Task,
        creation_token: &str,
        claim_ttl_ms: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if claim_ttl_ms == 0 {
            return Err(integrity("Creation claim TTL must be positive"));
        }
        let params_json = task.params.as_ref().map(serde_json::to_value).transpose()?;
        let result_json = task.result.as_ref().map(serde_json::to_value).transpose()?;
        let error_json = task.error.as_ref().map(serde_json::to_value).transpose()?;
        let metadata_json = task
            .metadata
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let auth_config_json = task
            .auth_config
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let webhooks_json = task
            .webhooks
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let cleanup_json = task
            .cleanup
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let tags_json = task.tags.as_ref().map(serde_json::to_value).transpose()?;
        let assign_mode = task
            .assign_mode
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .and_then(|value| value.as_str().map(str::to_string));
        let disconnect_policy = task
            .disconnect_policy
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .and_then(|value| value.as_str().map(str::to_string));
        let status = serde_json::to_value(&task.status)?
            .as_str()
            .unwrap_or("pending")
            .to_string();
        let sql = format!(
            r#"
            INSERT INTO {TASKS} (
                id, type, status, params, result, error, metadata,
                auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
                tags, assign_mode, cost, assigned_worker, disconnect_policy,
                creation_token, creation_claimed_at, creation_claim_expires_at,
                creation_completed_at, execution_deadline_at, task_version
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19, $20,
                FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT,
                FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT + $21,
                NULL,
                CASE
                    WHEN $22
                    THEN FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
                        + $23 * 1000
                    ELSE NULL
                END,
                0
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
                creation_completed_at = NULL,
                execution_deadline_at = EXCLUDED.execution_deadline_at,
                task_version = 0,
                ttl_claim_token = NULL,
                ttl_claim_until = NULL
            WHERE {TASKS}.creation_token IS NOT NULL
              AND {TASKS}.creation_completed_at IS NULL
              AND (
                {TASKS}.creation_claim_expires_at IS NULL
                OR {TASKS}.creation_claim_expires_at <=
                  FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
              )
              AND {TASKS}.status = 'pending'
              AND {TASKS}.updated_at = {TASKS}.created_at
              AND {TASKS}.result IS NULL
              AND {TASKS}.error IS NULL
              AND {TASKS}.completed_at IS NULL
              AND {TASKS}.storage_state = 'hot'
              AND {TASKS}.storage_epoch = 1
              AND {TASKS}.active_release_generation IS NULL
              AND {TASKS}.archive_watermark = -1
              AND {TASKS}.last_event_at IS NULL
              AND {TASKS}.cold_at IS NULL
              AND {TASKS}.task_version = 0
              AND NOT EXISTS (
                SELECT 1 FROM {EVENTS} AS event WHERE event.task_id = {TASKS}.id
              )
            "#
        );
        let result = sqlx::query(&sql)
            .bind(&task.id)
            .bind(&task.r#type)
            .bind(status)
            .bind(params_json)
            .bind(result_json)
            .bind(error_json)
            .bind(metadata_json)
            .bind(auth_config_json)
            .bind(webhooks_json)
            .bind(cleanup_json)
            .bind(task.created_at as i64)
            .bind(task.updated_at as i64)
            .bind(task.completed_at.map(|value| value as i64))
            .bind(task.ttl.map(|value| value as i32))
            .bind(tags_json)
            .bind(assign_mode)
            .bind(task.cost.map(|value| value as i32))
            .bind(&task.assigned_worker)
            .bind(disconnect_policy)
            .bind(creation_token)
            .bind(claim_ttl_ms as i64)
            .bind(has_execution_deadline(&task))
            .bind(task.ttl.unwrap_or(0) as i64)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn complete_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "UPDATE {TASKS} \
             SET creation_completed_at = COALESCE(\
                   creation_completed_at, \
                   FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT\
                 ), \
                 creation_claim_expires_at = NULL \
             WHERE id = $1 AND creation_token = $2"
        );
        let result = sqlx::query(&sql)
            .bind(task_id)
            .bind(creation_token)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn abort_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            r#"
            DELETE FROM {TASKS} AS task
            WHERE task.id = $1
              AND task.creation_token = $2
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
                SELECT 1 FROM {EVENTS} AS event WHERE event.task_id = task.id
              )
            "#
        );
        let result = sqlx::query(&sql)
            .bind(task_id)
            .bind(creation_token)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
            let sql = format!("SELECT * FROM {TASKS} WHERE id = $1");

            let row = sqlx::query(&sql)
                .bind(task_id)
                .fetch_optional(&self.pool)
                .await?;

            Ok(row.as_ref().map(Self::row_to_task))
        })
        .await
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
        let sql = format!(
            r#"
            INSERT INTO {EVENTS} (
                id, task_id, idx, timestamp, type, level, data, series_id, series_mode, series_acc_field
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
            )
            ON CONFLICT (id) DO NOTHING
            "#
        );

        let level_str =
            serde_json::to_value(&event.level).map(|v| v.as_str().unwrap_or("info").to_string())?;
        let series_mode_str: Option<String> = event.series_mode.as_ref().and_then(|sm| {
            serde_json::to_value(sm)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        });

        let idx = event.index as i32;
        let timestamp = event.timestamp as i64;
        let data_json: Option<JsonValue> = if event.data.is_null() {
            None
        } else {
            Some(event.data.clone())
        };

        sqlx::query(&sql)
            .bind(&event.id)
            .bind(&event.task_id)
            .bind(idx)
            .bind(timestamp)
            .bind(&event.r#type)
            .bind(&level_str)
            .bind(&data_json)
            .bind(&event.series_id)
            .bind(&series_mode_str)
            .bind(&event.series_acc_field)
            .execute(&self.pool)
            .await?;

        Ok(())
        })
        .await
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
            let mode = series_mode_to_string(&SeriesMode::Latest).unwrap();
            let mut tx = self.pool.begin().await?;
            let archive_watermark = lock_task_for_series_write_pg_tx(&mut tx, task_id).await?;
            let committed = get_series_state_for_update_pg_tx(&mut tx, task_id, series_id).await?;
            if archive_watermark >= event.index as i64
                || committed
                    .as_ref()
                    .is_some_and(|state| state.through_index >= event.index)
            {
                if archive_watermark >= event.index as i64 && committed.is_none() {
                    return Err(integrity(&format!(
                        "Archived latest series state is missing for {task_id}:{series_id}"
                    )));
                }
                tx.commit().await?;
                return Ok(());
            }
            if committed.as_ref().is_some_and(|state| {
                state.mode != SeriesMode::Latest
                    || state.event.series_acc_field != event.series_acc_field
            }) {
                return Err(integrity(&format!(
                    "Durable series semantics conflict for {task_id}:{series_id}"
                )));
            }
            let sql = format!(
                r#"
            SELECT * FROM {EVENTS}
            WHERE task_id = $1 AND series_id = $2 AND series_mode = $3
            ORDER BY idx ASC
            "#
            );
            let rows = sqlx::query(&sql)
                .bind(task_id)
                .bind(series_id)
                .bind(&mode)
                .fetch_all(&mut *tx)
                .await?;

            if let Some(existing) = rows.first().map(Self::row_to_event) {
                update_stored_series_event_pg(&mut tx, &existing, &event).await?;
                let sql = format!(
                    r#"
                DELETE FROM {EVENTS}
                WHERE task_id = $1 AND series_id = $2 AND series_mode = $3 AND id <> $4
                "#
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(series_id)
                    .bind(&mode)
                    .bind(&existing.id)
                    .execute(&mut *tx)
                    .await?;
            } else {
                insert_event_pg_tx(&mut tx, &event).await?;
            }
            save_series_state_pg_tx(
                &mut tx,
                &DurableSeriesState {
                    task_id: task_id.to_string(),
                    series_id: series_id.to_string(),
                    mode: SeriesMode::Latest,
                    event: event.clone(),
                    through_index: event.index,
                },
            )
            .await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
            let mode = series_mode_to_string(&SeriesMode::Accumulate).unwrap();
            let source_index = event.index;
            let mut tx = self.pool.begin().await?;
            let archive_watermark = lock_task_for_series_write_pg_tx(&mut tx, task_id).await?;
            let committed = get_series_state_for_update_pg_tx(&mut tx, task_id, series_id).await?;
            if archive_watermark >= event.index as i64
                || committed
                    .as_ref()
                    .is_some_and(|state| state.through_index >= event.index)
            {
                let Some(committed) = committed else {
                    return Err(integrity(&format!(
                        "Archived accumulate series state is missing for {task_id}:{series_id}"
                    )));
                };
                tx.commit().await?;
                return Ok(committed.event);
            }
            if committed.as_ref().is_some_and(|state| {
                state.mode != SeriesMode::Accumulate
                    || state.event.series_acc_field.as_deref().unwrap_or("delta") != field
                    || event.series_acc_field.as_deref().unwrap_or("delta") != field
            }) {
                return Err(integrity(&format!(
                    "Durable series semantics conflict for {task_id}:{series_id}"
                )));
            }
            let sql = format!(
                r#"
            SELECT * FROM {EVENTS}
            WHERE task_id = $1 AND series_id = $2 AND series_mode = $3
            ORDER BY idx ASC
            "#
            );
            let rows = sqlx::query(&sql)
                .bind(task_id)
                .bind(series_id)
                .bind(&mode)
                .fetch_all(&mut *tx)
                .await?;

            let first = rows.first().map(Self::row_to_event);
            let previous = rows.last().map(Self::row_to_event);
            let accumulated = if let Some(previous) = previous {
                accumulate_task_event(&previous, event, field)
            } else {
                event
            };

            if let Some(first) = first {
                update_stored_series_event_pg(&mut tx, &first, &accumulated).await?;
                let sql = format!(
                    r#"
                DELETE FROM {EVENTS}
                WHERE task_id = $1 AND series_id = $2 AND series_mode = $3 AND id <> $4
                "#
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(series_id)
                    .bind(&mode)
                    .bind(&first.id)
                    .execute(&mut *tx)
                    .await?;
            } else {
                insert_event_pg_tx(&mut tx, &accumulated).await?;
            }
            save_series_state_pg_tx(
                &mut tx,
                &DurableSeriesState {
                    task_id: task_id.to_string(),
                    series_id: series_id.to_string(),
                    mode: SeriesMode::Accumulate,
                    event: accumulated.clone(),
                    through_index: source_index,
                },
            )
            .await?;

            tx.commit().await?;
            Ok(accumulated)
        })
        .await
    }

    async fn get_task_storage_metadata(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskStorageMetadata>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT id, storage_state, storage_epoch, active_release_generation, \
             archive_watermark, last_event_at, cold_at, execution_deadline_at, task_version \
             FROM {TASKS} WHERE id = $1"
        );
        let row = sqlx::query(&sql)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_storage_metadata).transpose()
    }

    async fn claim_overdue_tasks(
        &self,
        limit: u64,
        claim_ttl_ms: u64,
    ) -> Result<Vec<TtlClaim>, Box<dyn std::error::Error + Send + Sync>> {
        validate_positive_i64(limit, "TTL claim limit")?;
        validate_positive_i64(claim_ttl_ms, "TTL claim duration")?;
        let sql = format!(
            r#"
            WITH overdue AS (
                SELECT id
                FROM {TASKS}
                WHERE execution_deadline_at IS NOT NULL
                  AND execution_deadline_at <=
                    FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
                  AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled')
                  AND (
                    ttl_claim_until IS NULL
                    OR ttl_claim_until <=
                      FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
                  )
                ORDER BY execution_deadline_at, id
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE {TASKS} AS task
            SET ttl_claim_token = MD5(
                    task.id || ':' || clock_timestamp()::TEXT || ':'
                    || random()::TEXT || ':' || txid_current()::TEXT
                ),
                ttl_claim_until =
                  FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT + $2
            FROM overdue
            WHERE task.id = overdue.id
            RETURNING task.id, task.ttl_claim_token, task.ttl_claim_until,
                      task.task_version, task.execution_deadline_at
            "#
        );
        let rows = sqlx::query(&sql)
            .bind(limit as i64)
            .bind(claim_ttl_ms as i64)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|row| TtlClaim {
                task_id: row.get("id"),
                claim_token: row.get("ttl_claim_token"),
                claim_until: row.get::<i64, _>("ttl_claim_until") as f64,
                task_version: row.get::<i64, _>("task_version") as u64,
                execution_deadline_at: row.get::<i64, _>("execution_deadline_at") as f64,
            })
            .collect())
    }

    async fn terminalize_ttl_claim(
        &self,
        claim: TtlClaim,
        task: Task,
        event: TaskEvent,
        assignment: Option<WorkerAssignment>,
    ) -> Result<Option<TerminalProjection>, Box<dyn std::error::Error + Send + Sync>> {
        validate_ttl_terminalization(&claim, &task, &event)?;
        let mut tx = self.pool.begin().await?;
        let now: i64 = sqlx::query_scalar(
            "SELECT FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT",
        )
        .fetch_one(&mut *tx)
        .await?;
        let task_sql = format!("SELECT * FROM {TASKS} WHERE id = $1 FOR UPDATE");
        let current = sqlx::query(&task_sql)
            .bind(&claim.task_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(current) = current else {
            tx.rollback().await?;
            return Ok(None);
        };
        let claim_token: Option<String> = current.get("ttl_claim_token");
        let claim_until: Option<i64> = current.get("ttl_claim_until");
        let deadline: Option<i64> = current.get("execution_deadline_at");
        let version: i64 = current.get("task_version");
        let status: String = current.get("status");
        if claim_token.as_deref() != Some(claim.claim_token.as_str())
            || claim_until != Some(claim.claim_until as i64)
            || claim_until.is_none_or(|until| until <= now)
            || version != claim.task_version as i64
            || deadline != Some(claim.execution_deadline_at as i64)
            || is_terminal_db_status(&status)
        {
            tx.rollback().await?;
            return Ok(None);
        }

        let assignment_sql =
            format!("SELECT * FROM {DURABLE_ASSIGNMENTS} WHERE task_id = $1 FOR UPDATE");
        let durable_assignment = sqlx::query(&assignment_sql)
            .bind(&claim.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .as_ref()
            .map(row_to_worker_assignment)
            .transpose()?;
        if durable_assignment != assignment {
            return Err(integrity(&format!(
                "Durable assignment changed before TTL terminalization: {}",
                claim.task_id
            )));
        }

        let index_sql = format!("SELECT COALESCE(MAX(idx), -1) FROM {EVENTS} WHERE task_id = $1");
        let last_index: i32 = sqlx::query_scalar(&index_sql)
            .bind(&claim.task_id)
            .fetch_one(&mut *tx)
            .await?;
        let expected_index = (i64::from(last_index) + 1) as u64;
        if event.index != expected_index {
            return Err(integrity(&format!(
                "TTL timeout event index is not contiguous for {}",
                claim.task_id
            )));
        }

        let update_sql = format!(
            r#"
            UPDATE {TASKS}
            SET status = 'timeout', result = $1, error = $2, metadata = $3,
                updated_at = $4, completed_at = $5, assigned_worker = NULL,
                execution_deadline_at = NULL, task_version = task_version + 1,
                ttl_claim_token = NULL, ttl_claim_until = NULL
            WHERE id = $6
            "#
        );
        sqlx::query(&update_sql)
            .bind(task.result.as_ref().map(serde_json::to_value).transpose()?)
            .bind(task.error.as_ref().map(serde_json::to_value).transpose()?)
            .bind(
                task.metadata
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?,
            )
            .bind(task.updated_at as i64)
            .bind(task.completed_at.map(|value| value as i64))
            .bind(&claim.task_id)
            .execute(&mut *tx)
            .await?;
        insert_event_pg_tx(&mut tx, &event).await?;
        let delete_assignment_sql = format!("DELETE FROM {DURABLE_ASSIGNMENTS} WHERE task_id = $1");
        sqlx::query(&delete_assignment_sql)
            .bind(&claim.task_id)
            .execute(&mut *tx)
            .await?;

        let projection = TerminalProjection {
            projection_id: format!("ttl:{}", event.id),
            task,
            event,
            assignment: durable_assignment,
            claim_token: Some(claim.claim_token.clone()),
            claim_until: Some(claim.claim_until),
        };
        let payload = TerminalProjectionPayload {
            task: projection.task.clone(),
            event: projection.event.clone(),
            assignment: projection.assignment.clone(),
        };
        let outbox_sql = format!(
            r#"
            INSERT INTO {TERMINAL_OUTBOX} (
                projection_id, task_id, event_id, assignment_id, payload,
                claim_token, claim_until, projected_at, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, $8)
            "#
        );
        sqlx::query(&outbox_sql)
            .bind(&projection.projection_id)
            .bind(&claim.task_id)
            .bind(&projection.event.id)
            .bind(projection.assignment.as_ref().map(durable_assignment_id))
            .bind(serde_json::to_value(payload)?)
            .bind(&claim.claim_token)
            .bind(claim.claim_until as i64)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(projection))
    }

    async fn claim_terminal_projections(
        &self,
        limit: u64,
        claim_token: &str,
        claim_ttl_ms: u64,
    ) -> Result<Vec<TerminalProjection>, Box<dyn std::error::Error + Send + Sync>> {
        validate_positive_i64(limit, "Terminal projection limit")?;
        validate_positive_i64(claim_ttl_ms, "Terminal projection claim duration")?;
        if claim_token.is_empty() {
            return Err(integrity("Terminal projection claim token is required"));
        }
        let sql = format!(
            r#"
            WITH pending AS (
                SELECT projection_id
                FROM {TERMINAL_OUTBOX}
                WHERE projected_at IS NULL
                  AND (
                    claim_until IS NULL
                    OR claim_until <=
                      FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
                  )
                ORDER BY created_at, projection_id
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE {TERMINAL_OUTBOX} AS outbox
            SET claim_token = $2,
                claim_until =
                  FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT + $3
            FROM pending
            WHERE outbox.projection_id = pending.projection_id
            RETURNING outbox.projection_id, outbox.payload,
                      outbox.claim_token, outbox.claim_until
            "#
        );
        let rows = sqlx::query(&sql)
            .bind(limit as i64)
            .bind(claim_token)
            .bind(claim_ttl_ms as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_terminal_projection).collect()
    }

    async fn complete_terminal_projection(
        &self,
        projection: &TerminalProjection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (Some(claim_token), Some(claim_until)) =
            (&projection.claim_token, projection.claim_until)
        else {
            return Err(integrity(
                "Terminal projection completion requires an active claim",
            ));
        };
        let sql = format!(
            r#"
            UPDATE {TERMINAL_OUTBOX}
            SET projected_at =
                  FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT,
                claim_token = NULL, claim_until = NULL
            WHERE projection_id = $1
              AND projected_at IS NULL
              AND claim_token = $2
              AND claim_until = $3
              AND claim_until >
                FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
            "#
        );
        let result = sqlx::query(&sql)
            .bind(&projection.projection_id)
            .bind(claim_token)
            .bind(claim_until as i64)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 1 {
            return Ok(());
        }
        let check_sql =
            format!("SELECT projected_at FROM {TERMINAL_OUTBOX} WHERE projection_id = $1");
        let projected_at: Option<i64> = sqlx::query_scalar(&check_sql)
            .bind(&projection.projection_id)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        if projected_at.is_some() {
            return Ok(());
        }
        Err(Box::new(StorageFenceConflictError::new(format!(
            "Terminal projection claim was lost: {}",
            projection.projection_id
        ))))
    }

    async fn save_durable_assignment(
        &self,
        assignment: WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        validate_worker_assignment(&assignment)?;
        let status = serde_json::to_value(&assignment.status)?
            .as_str()
            .unwrap_or("assigned")
            .to_string();
        let sql = format!(
            r#"
            INSERT INTO {DURABLE_ASSIGNMENTS} (
                task_id, assignment_id, worker_id, cost, assigned_at, status, updated_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6,
                FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
            )
            ON CONFLICT (task_id) DO UPDATE SET
                assignment_id = EXCLUDED.assignment_id,
                worker_id = EXCLUDED.worker_id,
                cost = EXCLUDED.cost,
                assigned_at = EXCLUDED.assigned_at,
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#
        );
        sqlx::query(&sql)
            .bind(&assignment.task_id)
            .bind(durable_assignment_id(&assignment))
            .bind(&assignment.worker_id)
            .bind(assignment.cost as i32)
            .bind(assignment.assigned_at as i64)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_durable_assignment(
        &self,
        task_id: &str,
        assignment_id: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sql = if assignment_id.is_some() {
            format!("DELETE FROM {DURABLE_ASSIGNMENTS} WHERE task_id = $1 AND assignment_id = $2")
        } else {
            format!("DELETE FROM {DURABLE_ASSIGNMENTS} WHERE task_id = $1")
        };
        let mut query = sqlx::query(&sql).bind(task_id);
        if let Some(assignment_id) = assignment_id {
            query = query.bind(assignment_id);
        }
        query.execute(&self.pool).await?;
        Ok(())
    }

    async fn persist_storage_release_request(
        &self,
        request: StorageReleaseRequest,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        validate_storage_release_request(&request)?;
        let sql = format!(
            "UPDATE {TASKS} \
             SET release_requested_at = $1, release_expected_index = $2, \
                 release_inactive_since = $3 \
             WHERE id = $4"
        );
        let result = sqlx::query(&sql)
            .bind(request.requested_at as i64)
            .bind(request.expected_last_event_index)
            .bind(request.inactive_since as i64)
            .bind(&request.task_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn clear_storage_release_request(
        &self,
        request: &StorageReleaseRequest,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        validate_storage_release_request(request)?;
        let sql = format!(
            "UPDATE {TASKS} \
             SET release_requested_at = NULL, release_expected_index = NULL, \
                 release_inactive_since = NULL \
             WHERE id = $1 AND release_requested_at = $2 \
               AND release_expected_index = $3 AND release_inactive_since = $4"
        );
        let result = sqlx::query(&sql)
            .bind(&request.task_id)
            .bind(request.requested_at as i64)
            .bind(request.expected_last_event_index)
            .bind(request.inactive_since as i64)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn list_storage_release_requests(
        &self,
        limit: u64,
    ) -> Result<Vec<StorageReleaseRequest>, Box<dyn std::error::Error + Send + Sync>> {
        if limit == 0 || limit > i64::MAX as u64 {
            return Err(integrity("Storage release request limit must be positive"));
        }
        let sql = format!(
            "SELECT id, release_requested_at, release_expected_index, release_inactive_since \
             FROM {TASKS} \
             WHERE release_requested_at IS NOT NULL \
               AND release_expected_index IS NOT NULL \
               AND release_inactive_since IS NOT NULL \
             ORDER BY release_requested_at, id LIMIT $1"
        );
        let rows = sqlx::query(&sql)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|row| StorageReleaseRequest {
                task_id: row.get("id"),
                requested_at: row.get::<i64, _>("release_requested_at") as f64,
                expected_last_event_index: row.get("release_expected_index"),
                inactive_since: row.get::<i64, _>("release_inactive_since") as f64,
            })
            .collect())
    }

    async fn compare_and_set_task_storage_metadata(
        &self,
        update: TaskStorageMetadataCas,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        validate_storage_metadata_cas(&update)?;
        let sql = format!(
            r#"
            UPDATE {TASKS}
            SET storage_state = $1, storage_epoch = $2, active_release_generation = $3,
                archive_watermark = $4, last_event_at = $5, cold_at = $6,
                execution_deadline_at = $7, task_version = $8
            WHERE id = $9
              AND storage_state = $10
              AND storage_epoch = $11
              AND active_release_generation IS NOT DISTINCT FROM $12
              AND archive_watermark = $4
              AND task_version <= $8
            "#
        );
        let result = sqlx::query(&sql)
            .bind(storage_state_to_string(&update.next.storage_state)?)
            .bind(update.next.storage_epoch as i64)
            .bind(&update.next.active_release_generation)
            .bind(update.next.archive_watermark)
            .bind(update.next.last_event_at.map(|value| value as i64))
            .bind(update.next.cold_at.map(|value| value as i64))
            .bind(update.next.execution_deadline_at.map(|value| value as i64))
            .bind(update.next.task_version as i64)
            .bind(&update.task_id)
            .bind(storage_state_to_string(&update.expected_storage_state)?)
            .bind(update.expected_storage_epoch as i64)
            .bind(&update.expected_release_generation)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn begin_archive(
        &self,
        generation: ArchiveGeneration,
    ) -> Result<ArchiveGeneration, Box<dyn std::error::Error + Send + Sync>> {
        validate_archive_manifest(&generation.manifest)?;
        if generation.storage_epoch == 0 || generation.storage_epoch > i64::MAX as u64 {
            return Err(integrity("Archive generation storage epoch is invalid"));
        }
        if generation.status != ArchiveGenerationStatus::Open {
            return Err(integrity("A new archive generation must start open"));
        }

        let mut tx = self.pool.begin().await?;
        let task_sql = format!(
            "SELECT storage_state, storage_epoch, active_release_generation, archive_watermark \
             FROM {TASKS} WHERE id = $1 FOR UPDATE"
        );
        let task = sqlx::query(&task_sql)
            .bind(&generation.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                integrity(&format!(
                    "Archive task does not exist: {}",
                    generation.task_id
                ))
            })?;

        let existing_sql = format!(
            "SELECT * FROM {ARCHIVE_GENERATIONS} \
             WHERE task_id = $1 AND generation = $2 FOR UPDATE"
        );
        if let Some(row) = sqlx::query(&existing_sql)
            .bind(&generation.task_id)
            .bind(&generation.generation)
            .fetch_optional(&mut *tx)
            .await?
        {
            let existing = row_to_archive_generation(&row)?;
            if existing.storage_epoch != generation.storage_epoch
                || existing.target_watermark != generation.target_watermark
                || existing.manifest != generation.manifest
            {
                return Err(integrity(
                    "Archive generation replay conflicts with durable manifest",
                ));
            }
            tx.commit().await?;
            return Ok(existing);
        }

        let task_state: String = task.get("storage_state");
        let task_epoch: i64 = task.get("storage_epoch");
        let active_generation: Option<String> = task.get("active_release_generation");
        let archive_watermark: i64 = task.get("archive_watermark");
        if task_state != "releasing"
            || task_epoch != generation.storage_epoch as i64
            || active_generation.as_deref() != Some(generation.generation.as_str())
        {
            return Err(integrity(
                "Archive generation is stale for the active task release",
            ));
        }
        if generation.manifest.target_watermark != generation.target_watermark
            || generation.manifest.prior_watermark != archive_watermark
            || generation.target_watermark < archive_watermark
        {
            return Err(integrity(
                "Archive manifest watermark conflicts with durable task state",
            ));
        }

        let insert_sql = format!(
            r#"
            INSERT INTO {ARCHIVE_GENERATIONS} (
                task_id, generation, storage_epoch, target_watermark, manifest,
                status, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, 'uploading', $6, $7)
            "#
        );
        sqlx::query(&insert_sql)
            .bind(&generation.task_id)
            .bind(&generation.generation)
            .bind(generation.storage_epoch as i64)
            .bind(generation.target_watermark)
            .bind(serde_json::to_value(&generation.manifest)?)
            .bind(generation.created_at as i64)
            .bind(generation.updated_at as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(generation)
    }

    async fn archive_batch(
        &self,
        task_id: &str,
        generation: &str,
        batch: ArchiveBatch,
    ) -> Result<ArchiveBatchReceipt, Box<dyn std::error::Error + Send + Sync>> {
        validate_batch_shape(task_id, generation, &batch)?;
        let computed_batch_digest = compute_archive_batch_digest(
            batch.receipt.previous_batch_digest.as_deref(),
            &batch.events,
            &batch.series_latest,
        )?;
        if computed_batch_digest != batch.receipt.batch_digest {
            return Err(integrity(
                "Archive batch digest does not match its contents",
            ));
        }
        let source_index_digest = compute_archive_source_page_digest(&batch.events)?;
        let source_series_digest = compute_series_state_digest(&batch.series_latest)?;
        let series_coverage = build_batch_series_coverage(&batch)?;

        let mut tx = self.pool.begin().await?;
        let task_sql = format!(
            "SELECT storage_state, storage_epoch, active_release_generation \
             FROM {TASKS} WHERE id = $1 FOR UPDATE"
        );
        let task_row = sqlx::query(&task_sql)
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| integrity(&format!("Archive task does not exist: {task_id}")))?;
        let generation_sql = format!(
            "SELECT * FROM {ARCHIVE_GENERATIONS} \
             WHERE task_id = $1 AND generation = $2 FOR UPDATE"
        );
        let generation_row = sqlx::query(&generation_sql)
            .bind(task_id)
            .bind(generation)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| integrity("Archive generation does not exist"))?;
        let archive = row_to_archive_generation(&generation_row)?;
        let task_state: String = task_row.get("storage_state");
        let task_epoch: i64 = task_row.get("storage_epoch");
        let active_generation: Option<String> = task_row.get("active_release_generation");
        if task_state != "releasing"
            || task_epoch != archive.storage_epoch as i64
            || active_generation.as_deref() != Some(generation)
        {
            return Err(integrity(
                "Archive batch lost its active task release fence",
            ));
        }
        validate_series_state_bounds(&batch.series_latest, archive.target_watermark)?;

        let existing_sql = format!(
            "SELECT * FROM {ARCHIVE_BATCHES} \
             WHERE task_id = $1 AND generation = $2 AND ordinal = $3"
        );
        if let Some(row) = sqlx::query(&existing_sql)
            .bind(task_id)
            .bind(generation)
            .bind(batch.receipt.ordinal as i32)
            .fetch_optional(&mut *tx)
            .await?
        {
            let existing = row_to_archive_batch_receipt(&row);
            let stored_source: String = row.get("source_index_digest");
            let stored_series: String = row.get("source_series_digest");
            let stored_coverage: JsonValue = row.get("series_coverage");
            let stored_coverage =
                serde_json::from_value::<Vec<ArchiveSeriesCoverage>>(stored_coverage)?;
            if existing != batch.receipt
                || stored_source != source_index_digest
                || stored_series != source_series_digest
                || stored_coverage != series_coverage
            {
                return Err(integrity(
                    "Archive batch replay conflicts with durable receipt",
                ));
            }
            tx.commit().await?;
            return Ok(existing);
        }
        if archive.status != ArchiveGenerationStatus::Open {
            return Err(integrity(
                "Cannot append a batch to a finalized archive generation",
            ));
        }

        let receipts_sql = format!(
            "SELECT ordinal, current_digest, source_last_index FROM {ARCHIVE_BATCHES} \
             WHERE task_id = $1 AND generation = $2 ORDER BY ordinal ASC"
        );
        let receipt_rows = sqlx::query(&receipts_sql)
            .bind(task_id)
            .bind(generation)
            .fetch_all(&mut *tx)
            .await?;
        let expected_ordinal = archive
            .manifest
            .expected_batch_ordinals
            .get(receipt_rows.len())
            .copied();
        if expected_ordinal != Some(batch.receipt.ordinal) {
            return Err(integrity(
                "Archive batches must be uploaded once in manifest order",
            ));
        }
        let expected_previous = receipt_rows
            .last()
            .map(|row| row.get::<String, _>("current_digest"));
        if batch.receipt.previous_batch_digest != expected_previous {
            return Err(integrity(
                "Archive batch previous digest breaks the receipt chain",
            ));
        }
        let previous_last_index = receipt_rows
            .last()
            .and_then(|row| row.get::<Option<i64>, _>("source_last_index"))
            .unwrap_or(archive.manifest.prior_watermark);
        if batch
            .receipt
            .first_index
            .is_none_or(|index| index as i64 <= previous_last_index)
        {
            return Err(integrity(
                "Archive batch source coverage overlaps or goes backwards",
            ));
        }

        for event in &batch.events {
            if event.index as i64 <= archive.manifest.prior_watermark
                || event.index as i64 > archive.manifest.target_watermark
            {
                return Err(integrity(
                    "Archive batch event falls outside the sealed watermark",
                ));
            }
            if matches!(
                event.series_mode,
                Some(SeriesMode::Latest | SeriesMode::Accumulate)
            ) {
                assert_compact_event_compatible_pg_tx(&mut tx, event).await?;
                continue;
            }
            upsert_canonical_event_pg_tx(&mut tx, event).await?;
        }

        let insert_sql = format!(
            r#"
            INSERT INTO {ARCHIVE_BATCHES} (
                task_id, generation, ordinal, previous_digest, current_digest,
                source_first_index, source_last_index, source_index_digest,
                source_series_digest, series_coverage, entry_count, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#
        );
        sqlx::query(&insert_sql)
            .bind(task_id)
            .bind(generation)
            .bind(batch.receipt.ordinal as i32)
            .bind(&batch.receipt.previous_batch_digest)
            .bind(&batch.receipt.batch_digest)
            .bind(batch.receipt.first_index.map(|value| value as i64))
            .bind(batch.receipt.last_index.map(|value| value as i64))
            .bind(&source_index_digest)
            .bind(&source_series_digest)
            .bind(serde_json::to_value(&series_coverage)?)
            .bind(batch.receipt.entry_count as i32)
            .bind(now_millis())
            .execute(&mut *tx)
            .await?;
        let update_sql = format!(
            "UPDATE {ARCHIVE_GENERATIONS} SET updated_at = $1 \
             WHERE task_id = $2 AND generation = $3"
        );
        sqlx::query(&update_sql)
            .bind(now_millis())
            .bind(task_id)
            .bind(generation)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(batch.receipt)
    }

    async fn finalize_archive(
        &self,
        task_id: &str,
        generation: &str,
        task: Task,
        series_latest: Vec<DurableSeriesState>,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        if task.id != task_id {
            return Err(integrity("Final archive task ID does not match"));
        }
        validate_series_state(task_id, &series_latest)?;
        let series_digest = compute_series_state_digest(&series_latest)?;

        let mut tx = self.pool.begin().await?;
        let task_sql = format!("SELECT * FROM {TASKS} WHERE id = $1 FOR UPDATE");
        let task_row = sqlx::query(&task_sql)
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| integrity(&format!("Archive task does not exist: {task_id}")))?;
        let generation_sql = format!(
            "SELECT * FROM {ARCHIVE_GENERATIONS} \
             WHERE task_id = $1 AND generation = $2 FOR UPDATE"
        );
        let generation_row = sqlx::query(&generation_sql)
            .bind(task_id)
            .bind(generation)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| integrity("Archive generation does not exist"))?;
        let archive = row_to_archive_generation(&generation_row)?;
        validate_series_state_bounds(&series_latest, archive.target_watermark)?;
        let durable_watermark: i64 = task_row.get("archive_watermark");

        if archive.status == ArchiveGenerationStatus::Finalized {
            if durable_watermark < archive.target_watermark
                || series_digest != archive.manifest.series_state_digest
            {
                return Err(integrity(
                    "Finalized archive response replay failed verification",
                ));
            }
            tx.commit().await?;
            return Ok(archive.target_watermark);
        }
        if archive.status != ArchiveGenerationStatus::Open {
            return Err(integrity("Archive generation cannot be finalized"));
        }
        let task_state: String = task_row.get("storage_state");
        let task_epoch: i64 = task_row.get("storage_epoch");
        let active_generation: Option<String> = task_row.get("active_release_generation");
        if task_state != "releasing"
            || task_epoch != archive.storage_epoch as i64
            || active_generation.as_deref() != Some(generation)
        {
            return Err(integrity("Archive generation lost its task release fence"));
        }

        let receipts_sql = format!(
            "SELECT * FROM {ARCHIVE_BATCHES} WHERE task_id = $1 AND generation = $2 \
             ORDER BY ordinal ASC"
        );
        let receipt_rows = sqlx::query(&receipts_sql)
            .bind(task_id)
            .bind(generation)
            .fetch_all(&mut *tx)
            .await?;
        let receipts = receipt_rows
            .iter()
            .map(row_to_archive_batch_receipt)
            .collect::<Vec<_>>();
        let ordinals = receipts
            .iter()
            .map(|receipt| receipt.ordinal)
            .collect::<Vec<_>>();
        if ordinals != archive.manifest.expected_batch_ordinals {
            return Err(integrity(
                "Archive generation has missing or unexpected batch ordinals",
            ));
        }

        let mut previous_digest: Option<String> = None;
        let mut previous_last_index = archive.manifest.prior_watermark;
        let mut entry_count = 0_u64;
        let mut source_page_digests = Vec::with_capacity(receipts.len());
        let mut staged_series = std::collections::HashMap::<String, ArchiveSeriesCoverage>::new();
        for (receipt, row) in receipts.iter().zip(receipt_rows.iter()) {
            if receipt.previous_batch_digest != previous_digest {
                return Err(integrity(
                    "Archive generation contains a broken batch digest chain",
                ));
            }
            let (Some(first_index), Some(last_index)) = (receipt.first_index, receipt.last_index)
            else {
                return Err(integrity(
                    "Archive generation contains invalid source coverage",
                ));
            };
            if receipt.entry_count == 0
                || first_index as i64 <= previous_last_index
                || last_index < first_index
                || last_index as i64 > archive.target_watermark
            {
                return Err(integrity(
                    "Archive generation contains invalid source coverage",
                ));
            }
            previous_digest = Some(receipt.batch_digest.clone());
            previous_last_index = last_index as i64;
            entry_count += receipt.entry_count;
            source_page_digests.push(row.get::<String, _>("source_index_digest"));
            let coverage =
                serde_json::from_value::<Vec<ArchiveSeriesCoverage>>(row.get("series_coverage"))?;
            for current in coverage {
                validate_series_coverage(&current, archive.target_watermark)?;
                if let Some(previous) = staged_series.get(&current.series_id) {
                    if current.mode != previous.mode
                        || current.through_index < previous.through_index
                        || (current.through_index == previous.through_index && current != *previous)
                    {
                        return Err(integrity(&format!(
                            "Archive generation contains conflicting staged state for series {}",
                            current.series_id
                        )));
                    }
                }
                let should_replace = staged_series
                    .get(&current.series_id)
                    .is_none_or(|previous| current.through_index > previous.through_index);
                if should_replace {
                    staged_series.insert(current.series_id.clone(), current);
                }
            }
        }
        if entry_count != archive.manifest.source_entry_count {
            return Err(integrity(
                "Archive generation source entry count does not match manifest",
            ));
        }
        if entry_count > 0 && previous_last_index != archive.manifest.target_watermark {
            return Err(integrity(
                "Archive generation does not reach its target watermark",
            ));
        }
        if compute_archive_source_digest(&source_page_digests) != archive.manifest.source_digest {
            return Err(integrity(
                "Archive generation source coverage digest does not match",
            ));
        }
        if series_digest != archive.manifest.series_state_digest {
            return Err(integrity(
                "Archive generation series state digest does not match",
            ));
        }

        let final_series = series_latest
            .iter()
            .map(|state| (state.series_id.as_str(), state))
            .collect::<std::collections::HashMap<_, _>>();
        for (series_id, coverage) in &staged_series {
            let Some(final_state) = final_series.get(series_id.as_str()) else {
                return Err(integrity(&format!(
                    "Archive final state does not cover compact source series {series_id}"
                )));
            };
            if final_state.mode != coverage.mode
                || final_state.through_index < coverage.through_index
            {
                return Err(integrity(&format!(
                    "Archive final state does not cover compact source series {series_id}"
                )));
            }
        }

        let committed_series_sql =
            format!("SELECT * FROM {SERIES_STATE} WHERE task_id = $1 FOR UPDATE");
        let committed_series_rows = sqlx::query(&committed_series_sql)
            .bind(task_id)
            .fetch_all(&mut *tx)
            .await?;
        let committed_series = committed_series_rows
            .iter()
            .map(row_to_durable_series_state)
            .collect::<Result<Vec<_>, _>>()?;
        for committed in &committed_series {
            let Some(final_state) = final_series.get(committed.series_id.as_str()) else {
                return Err(integrity(&format!(
                    "Archive final state regresses committed series {}",
                    committed.series_id
                )));
            };
            if final_state.mode != committed.mode
                || final_state.event.series_acc_field != committed.event.series_acc_field
                || final_state.through_index < committed.through_index
                || (final_state.through_index == committed.through_index
                    && durable_series_state_record(final_state)?
                        != durable_series_state_record(committed)?)
            {
                return Err(integrity(&format!(
                    "Archive final state regresses committed series {}",
                    committed.series_id
                )));
            }
        }
        let compact_event_sql = format!(
            "SELECT * FROM {EVENTS} WHERE task_id = $1 \
             AND series_mode IN ('latest', 'accumulate') FOR UPDATE"
        );
        let compact_event_rows = sqlx::query(&compact_event_sql)
            .bind(task_id)
            .fetch_all(&mut *tx)
            .await?;
        for row in &compact_event_rows {
            let existing = PostgresLongTermStore::row_to_event(row);
            let Some(series_id) = existing.series_id.as_deref() else {
                return Err(integrity(
                    "Archive final state omits committed compact series <missing>",
                ));
            };
            let Some(final_state) = final_series.get(series_id) else {
                return Err(integrity(&format!(
                    "Archive final state omits committed compact series {series_id}"
                )));
            };
            if final_state.event.series_mode != existing.series_mode
                || final_state.event.series_acc_field != existing.series_acc_field
                || final_state.through_index < existing.index
            {
                return Err(integrity(&format!(
                    "Archive final state omits committed compact series {series_id}"
                )));
            }
        }

        update_task_pg_tx(&mut tx, &task).await?;
        let delete_series_sql = format!("DELETE FROM {SERIES_STATE} WHERE task_id = $1");
        sqlx::query(&delete_series_sql)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        for state in &series_latest {
            let mode = series_mode_to_string(&state.mode)
                .ok_or_else(|| integrity("Durable series mode is not serializable"))?;
            let insert_series_sql = format!(
                "INSERT INTO {SERIES_STATE} \
                 (task_id, series_id, mode, event, through_index, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6)"
            );
            sqlx::query(&insert_series_sql)
                .bind(task_id)
                .bind(&state.series_id)
                .bind(&mode)
                .bind(serde_json::to_value(&state.event)?)
                .bind(state.through_index as i64)
                .bind(now_millis())
                .execute(&mut *tx)
                .await?;
            let committed = committed_series
                .iter()
                .find(|candidate| candidate.series_id == state.series_id);
            install_canonical_series_event_pg_tx(&mut tx, state, committed).await?;
        }

        let update_sql = format!(
            r#"
            UPDATE {TASKS}
            SET archive_watermark = GREATEST(archive_watermark, $1)
            WHERE id = $2 AND storage_state = 'releasing'
              AND storage_epoch = $3 AND active_release_generation = $4
            RETURNING archive_watermark
            "#
        );
        let updated = sqlx::query(&update_sql)
            .bind(archive.target_watermark)
            .bind(task_id)
            .bind(archive.storage_epoch as i64)
            .bind(generation)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| integrity("Archive task fence changed during finalization"))?;
        let watermark: i64 = updated.get("archive_watermark");
        let finalize_sql = format!(
            "UPDATE {ARCHIVE_GENERATIONS} SET status = 'finalized', finalized_at = $1, \
             updated_at = $2 WHERE task_id = $3 AND generation = $4"
        );
        let now = now_millis();
        sqlx::query(&finalize_sql)
            .bind(now)
            .bind(now)
            .bind(task_id)
            .bind(generation)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(watermark)
    }

    async fn get_archive_watermark(
        &self,
        task_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!("SELECT archive_watermark FROM {TASKS} WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| integrity(&format!("Task does not exist: {task_id}")))?;
        Ok(row.get("archive_watermark"))
    }

    async fn get_last_event_index(
        &self,
        task_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            r#"
            SELECT GREATEST(
                task.archive_watermark,
                COALESCE((SELECT MAX(event.idx) FROM {EVENTS} event WHERE event.task_id = task.id), -1),
                COALESCE((SELECT MAX(series.through_index) FROM {SERIES_STATE} series WHERE series.task_id = task.id), -1)
            ) AS last_index
            FROM {TASKS} task WHERE task.id = $1
            "#
        );
        let row = sqlx::query(&sql)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| row.get("last_index")).unwrap_or(-1))
    }

    async fn get_recent_events(
        &self,
        task_id: &str,
        limit: u64,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let sql = format!("SELECT * FROM {EVENTS} WHERE task_id = $1 ORDER BY idx DESC LIMIT $2");
        let mut events = sqlx::query(&sql)
            .bind(task_id)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(Self::row_to_event)
            .collect::<Vec<_>>();
        events.reverse();
        Ok(events)
    }

    async fn get_durable_series_state(
        &self,
        task_id: &str,
    ) -> Result<Vec<DurableSeriesState>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!("SELECT * FROM {SERIES_STATE} WHERE task_id = $1 ORDER BY series_id ASC");
        sqlx::query(&sql)
            .bind(task_id)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_durable_series_state)
            .collect()
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
        let since = opts.as_ref().and_then(|o| o.since.as_ref());
        let limit = opts.as_ref().and_then(|o| o.limit);

        // Use a bind parameter for LIMIT to prevent SQL injection.
        // When no limit is specified, use a very large value (i.e. effectively unlimited).
        let limit_val = limit.map(|l| l as i64).unwrap_or(i64::MAX);

        let rows = if let Some(since) = since {
            if let Some(index) = since.index {
                let sql = format!(
                    "SELECT * FROM {EVENTS} WHERE task_id = $1 AND idx > $2 ORDER BY idx ASC LIMIT $3"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(index as i32)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            } else if let Some(timestamp) = since.timestamp {
                let sql = format!(
                    "SELECT * FROM {EVENTS} WHERE task_id = $1 AND timestamp > $2 ORDER BY idx ASC LIMIT $3"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(timestamp as i64)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            } else if let Some(ref id) = since.id {
                // Look up the anchor event's idx, then fetch events after it
                let anchor_sql = format!("SELECT idx FROM {EVENTS} WHERE id = $1");
                let anchor_row = sqlx::query(&anchor_sql)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;
                let anchor_idx: i32 = anchor_row.as_ref().map(|r| r.get("idx")).unwrap_or(-1);

                let sql = format!(
                    "SELECT * FROM {EVENTS} WHERE task_id = $1 AND idx > $2 ORDER BY idx ASC LIMIT $3"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(anchor_idx)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                // since exists but has no usable cursor fields
                let sql =
                    format!("SELECT * FROM {EVENTS} WHERE task_id = $1 ORDER BY idx ASC LIMIT $2");
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            }
        } else {
            let sql =
                format!("SELECT * FROM {EVENTS} WHERE task_id = $1 ORDER BY idx ASC LIMIT $2");
            sqlx::query(&sql)
                .bind(task_id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
        };

        Ok(rows.iter().map(Self::row_to_event).collect())
        })
        .await
    }

    fn supports_series_compaction(&self) -> bool {
        true
    }

    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
            let action_str = serde_json::to_value(&event.action)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let data_json: Option<JsonValue> = event
                .data
                .as_ref()
                .and_then(|d| serde_json::to_value(d).ok());

            let sql = format!(
                r#"
            INSERT INTO {WORKER_EVENTS} (id, worker_id, timestamp, action, data)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (id) DO NOTHING
            "#
            );

            sqlx::query(&sql)
                .bind(&event.id)
                .bind(&event.worker_id)
                .bind(event.timestamp as i64)
                .bind(&action_str)
                .bind(&data_json)
                .execute(&self.pool)
                .await?;

            Ok(())
        })
        .await
    }

    async fn get_worker_events(
        &self,
        worker_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.observed(|| async move {
        let since = opts.as_ref().and_then(|o| o.since.as_ref());
        let limit = opts.as_ref().and_then(|o| o.limit);
        let limit_val = limit.map(|l| l as i64).unwrap_or(i64::MAX);

        let rows = if let Some(since) = since {
            if let Some(timestamp) = since.timestamp {
                let sql = format!(
                    "SELECT * FROM {WORKER_EVENTS} WHERE worker_id = $1 AND timestamp > $2 ORDER BY timestamp ASC LIMIT $3"
                );
                sqlx::query(&sql)
                    .bind(worker_id)
                    .bind(timestamp as i64)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            } else if let Some(ref id) = since.id {
                // Look up the anchor event's timestamp, then fetch events after it
                let anchor_sql = format!("SELECT timestamp FROM {WORKER_EVENTS} WHERE id = $1");
                let anchor_row = sqlx::query(&anchor_sql)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;
                let anchor_ts: i64 = anchor_row
                    .as_ref()
                    .map(|r| r.get("timestamp"))
                    .unwrap_or(-1);

                let sql = format!(
                    "SELECT * FROM {WORKER_EVENTS} WHERE worker_id = $1 AND (timestamp > $2 OR (timestamp = $2 AND id > $3)) ORDER BY timestamp ASC, id ASC LIMIT $4"
                );
                sqlx::query(&sql)
                    .bind(worker_id)
                    .bind(anchor_ts)
                    .bind(id)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                // since exists but has no usable cursor fields
                let sql = format!(
                    "SELECT * FROM {WORKER_EVENTS} WHERE worker_id = $1 ORDER BY timestamp ASC LIMIT $2"
                );
                sqlx::query(&sql)
                    .bind(worker_id)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            }
        } else {
            let sql = format!(
                "SELECT * FROM {WORKER_EVENTS} WHERE worker_id = $1 ORDER BY timestamp ASC LIMIT $2"
            );
            sqlx::query(&sql)
                .bind(worker_id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
        };

        Ok(rows.iter().map(Self::row_to_worker_event).collect())
        })
        .await
    }
}

fn integrity(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(StorageIntegrityError::new(message))
}

fn validate_archive_manifest(
    manifest: &ArchiveSourceManifest,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if manifest.target_watermark < manifest.prior_watermark {
        return Err(integrity("Archive manifest contains invalid source bounds"));
    }
    if (manifest.source_entry_count == 0 && manifest.target_watermark != manifest.prior_watermark)
        || (manifest.source_entry_count > 0
            && manifest.target_watermark == manifest.prior_watermark)
        || (manifest.source_entry_count == 0 && !manifest.expected_batch_ordinals.is_empty())
        || (manifest.source_entry_count > 0
            && (manifest.expected_batch_ordinals.is_empty()
                || manifest.expected_batch_ordinals.len() as u64 > manifest.source_entry_count))
    {
        return Err(integrity(
            "Archive manifest batch count is inconsistent with its source",
        ));
    }
    if manifest
        .expected_batch_ordinals
        .iter()
        .enumerate()
        .any(|(index, ordinal)| *ordinal != index as u64)
    {
        return Err(integrity(
            "Archive manifest batch ordinals must be contiguous from zero",
        ));
    }
    if !is_sha256(&manifest.source_digest) || !is_sha256(&manifest.series_state_digest) {
        return Err(integrity(
            "Archive manifest digests must be lowercase SHA-256",
        ));
    }
    Ok(())
}

fn validate_storage_metadata_cas(
    update: &TaskStorageMetadataCas,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let next = &update.next;
    if next.task_id != update.task_id {
        return Err(integrity(
            "Storage metadata task ID does not match CAS target",
        ));
    }
    if next.storage_epoch == 0
        || next.storage_epoch < update.expected_storage_epoch
        || next.storage_epoch > i64::MAX as u64
        || next.archive_watermark < -1
        || next.task_version > i64::MAX as u64
    {
        return Err(integrity(
            "Storage metadata CAS would violate a monotonic counter",
        ));
    }
    if (next.storage_state == StorageState::Releasing && next.active_release_generation.is_none())
        || (next.storage_state == StorageState::Hot && next.active_release_generation.is_some())
    {
        return Err(integrity(
            "Storage metadata release generation is inconsistent",
        ));
    }
    if [next.last_event_at, next.cold_at, next.execution_deadline_at]
        .into_iter()
        .flatten()
        .any(|timestamp| !fits_postgres_bigint(timestamp))
    {
        return Err(integrity(
            "Storage metadata timestamps must be PostgreSQL BIGINT values",
        ));
    }
    Ok(())
}

fn validate_storage_release_request(
    request: &StorageReleaseRequest,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if request.task_id.is_empty()
        || !request.requested_at.is_finite()
        || request.requested_at < 0.0
        || request.requested_at.fract() != 0.0
        || request.requested_at > i64::MAX as f64
        || request.expected_last_event_index < -1
        || !request.inactive_since.is_finite()
        || request.inactive_since < 0.0
        || request.inactive_since.fract() != 0.0
        || request.inactive_since > i64::MAX as f64
    {
        return Err(integrity("Storage release request is invalid"));
    }
    Ok(())
}

fn has_execution_deadline(task: &Task) -> bool {
    task.ttl.is_some()
        && task.status != TaskStatus::Paused
        && !matches!(
            task.status,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Timeout
                | TaskStatus::Cancelled
        )
}

fn is_terminal_db_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "timeout" | "cancelled")
}

fn validate_positive_i64(
    value: u64,
    label: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if value == 0 || value > i64::MAX as u64 {
        return Err(integrity(&format!("{label} must be a positive integer")));
    }
    Ok(())
}

fn validate_ttl_terminalization(
    claim: &TtlClaim,
    task: &Task,
    event: &TaskEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if claim.task_id.is_empty()
        || claim.claim_token.is_empty()
        || !fits_postgres_bigint(claim.claim_until)
        || claim.task_version > i64::MAX as u64
        || !fits_postgres_bigint(claim.execution_deadline_at)
        || task.id != claim.task_id
        || task.status != TaskStatus::Timeout
        || task.completed_at.is_none()
        || event.task_id != claim.task_id
        || event.id.is_empty()
        || event.r#type != "taskcast:status"
        || event.index > i32::MAX as u64
        || !fits_postgres_bigint(event.timestamp)
    {
        return Err(integrity("TTL terminalization input is invalid"));
    }
    Ok(())
}

fn validate_worker_assignment(
    assignment: &WorkerAssignment,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if assignment.task_id.is_empty()
        || assignment.worker_id.is_empty()
        || assignment.cost > i32::MAX as u32
        || !fits_postgres_bigint(assignment.assigned_at)
        || assignment.assigned_at < 0.0
    {
        return Err(integrity("Durable worker assignment is invalid"));
    }
    Ok(())
}

fn durable_assignment_id(assignment: &WorkerAssignment) -> String {
    format!(
        "{}:{}:{}",
        assignment.task_id, assignment.worker_id, assignment.assigned_at as i64
    )
}

fn row_to_worker_assignment(
    row: &PgRow,
) -> Result<WorkerAssignment, Box<dyn std::error::Error + Send + Sync>> {
    let status: String = row.get("status");
    let status = serde_json::from_value(JsonValue::String(status))
        .map_err(|_| integrity("Durable worker assignment status is invalid"))?;
    Ok(WorkerAssignment {
        task_id: row.get("task_id"),
        worker_id: row.get("worker_id"),
        cost: row.get::<i32, _>("cost") as u32,
        assigned_at: row.get::<i64, _>("assigned_at") as f64,
        status,
    })
}

fn row_to_terminal_projection(
    row: &PgRow,
) -> Result<TerminalProjection, Box<dyn std::error::Error + Send + Sync>> {
    let payload: JsonValue = row.get("payload");
    let payload: TerminalProjectionPayload = serde_json::from_value(payload)
        .map_err(|_| integrity("Terminal projection payload is invalid"))?;
    if payload.task.id != payload.event.task_id
        || payload
            .assignment
            .as_ref()
            .is_some_and(|assignment| assignment.task_id != payload.task.id)
    {
        return Err(integrity("Terminal projection payload is inconsistent"));
    }
    Ok(TerminalProjection {
        projection_id: row.get("projection_id"),
        task: payload.task,
        event: payload.event,
        assignment: payload.assignment,
        claim_token: row.get("claim_token"),
        claim_until: row
            .get::<Option<i64>, _>("claim_until")
            .map(|value| value as f64),
    })
}

fn validate_batch_shape(
    task_id: &str,
    generation: &str,
    batch: &ArchiveBatch,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let receipt = &batch.receipt;
    if receipt.task_id != task_id
        || receipt.generation != generation
        || receipt.entry_count != batch.events.len() as u64
        || receipt.ordinal > i32::MAX as u64
        || receipt.entry_count > i32::MAX as u64
        || batch.events.is_empty()
    {
        return Err(integrity(
            "Archive batch identity, count, or ordinal is invalid",
        ));
    }
    let first_index = batch.events.first().map(|event| event.index);
    let last_index = batch.events.last().map(|event| event.index);
    if receipt.first_index != first_index || receipt.last_index != last_index {
        return Err(integrity(
            "Archive batch receipt coverage does not match its events",
        ));
    }
    let mut previous_index = None;
    let mut event_ids = std::collections::HashSet::new();
    for event in &batch.events {
        if event.task_id != task_id
            || event.id.is_empty()
            || event.index > i32::MAX as u64
            || !fits_postgres_bigint(event.timestamp)
            || previous_index.is_some_and(|index| event.index <= index)
            || !event_ids.insert(event.id.as_str())
            || event.series_snapshot.is_some()
            || event._accumulated_data.is_some()
        {
            return Err(integrity(
                "Archive batch events must have unique, increasing identities",
            ));
        }
        previous_index = Some(event.index);
    }
    validate_series_state(task_id, &batch.series_latest)
}

fn validate_series_state(
    task_id: &str,
    states: &[DurableSeriesState],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut series_ids = std::collections::HashSet::new();
    for state in states {
        if state.task_id != task_id
            || state.event.task_id != task_id
            || state.event.series_id.as_deref() != Some(state.series_id.as_str())
            || state.event.series_mode.as_ref() != Some(&state.mode)
            || state.through_index < state.event.index
            || state.through_index > i32::MAX as u64
            || state.series_id.is_empty()
            || state.event.index > i32::MAX as u64
            || !fits_postgres_bigint(state.event.timestamp)
            || state.event.series_snapshot.is_some()
            || state.event._accumulated_data.is_some()
            || !matches!(state.mode, SeriesMode::Latest | SeriesMode::Accumulate)
            || !series_ids.insert(state.series_id.as_str())
        {
            return Err(integrity("Archive durable series state is inconsistent"));
        }
    }
    Ok(())
}

fn validate_series_state_bounds(
    states: &[DurableSeriesState],
    target_watermark: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if states.iter().any(|state| {
        state.event.index as i64 > target_watermark || state.through_index as i64 > target_watermark
    }) {
        return Err(integrity(
            "Archive durable series state exceeds the sealed watermark",
        ));
    }
    Ok(())
}

fn build_batch_series_coverage(
    batch: &ArchiveBatch,
) -> Result<Vec<ArchiveSeriesCoverage>, Box<dyn std::error::Error + Send + Sync>> {
    let mut by_series = std::collections::HashMap::<String, ArchiveSeriesCoverage>::new();
    for event in &batch.events {
        let Some(mode @ (SeriesMode::Latest | SeriesMode::Accumulate)) = event.series_mode.as_ref()
        else {
            continue;
        };
        let Some(series_id) = event.series_id.as_deref() else {
            return Err(integrity("Archive compact event is missing its series ID"));
        };
        if by_series
            .get(series_id)
            .is_some_and(|previous| previous.mode != *mode)
        {
            return Err(integrity(&format!(
                "Archive compact source changes mode for series {series_id}"
            )));
        }
        by_series.insert(
            series_id.to_string(),
            ArchiveSeriesCoverage {
                series_id: series_id.to_string(),
                mode: mode.clone(),
                through_index: by_series.get(series_id).map_or(event.index, |previous| {
                    previous.through_index.max(event.index)
                }),
            },
        );
    }
    let mut coverage = by_series.into_values().collect::<Vec<_>>();
    coverage.sort_by(|left, right| left.series_id.cmp(&right.series_id));
    Ok(coverage)
}

fn validate_series_coverage(
    coverage: &ArchiveSeriesCoverage,
    target_watermark: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if coverage.series_id.is_empty()
        || !matches!(coverage.mode, SeriesMode::Latest | SeriesMode::Accumulate)
        || coverage.through_index as i64 > target_watermark
    {
        return Err(integrity("Archive compact series coverage is invalid"));
    }
    Ok(())
}

fn row_to_storage_metadata(
    row: &PgRow,
) -> Result<TaskStorageMetadata, Box<dyn std::error::Error + Send + Sync>> {
    let storage_state: String = row.get("storage_state");
    Ok(TaskStorageMetadata {
        task_id: row.get("id"),
        storage_state: serde_json::from_value(JsonValue::String(storage_state))
            .map_err(|_| integrity("Durable storage state is invalid"))?,
        storage_epoch: row.get::<i64, _>("storage_epoch") as u64,
        active_release_generation: row.get("active_release_generation"),
        archive_watermark: row.get("archive_watermark"),
        last_event_at: row
            .get::<Option<i64>, _>("last_event_at")
            .map(|value| value as f64),
        cold_at: row
            .get::<Option<i64>, _>("cold_at")
            .map(|value| value as f64),
        execution_deadline_at: row
            .get::<Option<i64>, _>("execution_deadline_at")
            .map(|value| value as f64),
        task_version: row.get::<i64, _>("task_version") as u64,
    })
}

fn row_to_archive_generation(
    row: &PgRow,
) -> Result<ArchiveGeneration, Box<dyn std::error::Error + Send + Sync>> {
    let status: String = row.get("status");
    let manifest: JsonValue = row.get("manifest");
    Ok(ArchiveGeneration {
        task_id: row.get("task_id"),
        generation: row.get("generation"),
        storage_epoch: row.get::<i64, _>("storage_epoch") as u64,
        target_watermark: row.get("target_watermark"),
        manifest: serde_json::from_value(manifest)?,
        status: match status.as_str() {
            "uploading" => ArchiveGenerationStatus::Open,
            "finalized" => ArchiveGenerationStatus::Finalized,
            _ => ArchiveGenerationStatus::Aborted,
        },
        created_at: row.get::<i64, _>("created_at") as f64,
        updated_at: row.get::<i64, _>("updated_at") as f64,
    })
}

fn row_to_archive_batch_receipt(row: &PgRow) -> ArchiveBatchReceipt {
    ArchiveBatchReceipt {
        task_id: row.get("task_id"),
        generation: row.get("generation"),
        ordinal: row.get::<i32, _>("ordinal") as u64,
        previous_batch_digest: row.get("previous_digest"),
        batch_digest: row.get("current_digest"),
        entry_count: row.get::<i32, _>("entry_count") as u64,
        first_index: row
            .get::<Option<i64>, _>("source_first_index")
            .map(|value| value as u64),
        last_index: row
            .get::<Option<i64>, _>("source_last_index")
            .map(|value| value as u64),
    }
}

fn row_to_durable_series_state(
    row: &PgRow,
) -> Result<DurableSeriesState, Box<dyn std::error::Error + Send + Sync>> {
    let mode: String = row.get("mode");
    let event: JsonValue = row.get("event");
    Ok(DurableSeriesState {
        task_id: row.get("task_id"),
        series_id: row.get("series_id"),
        mode: serde_json::from_value(JsonValue::String(mode))?,
        event: serde_json::from_value(event)?,
        through_index: row.get::<i64, _>("through_index") as u64,
    })
}

async fn lock_task_for_series_write_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    task_id: &str,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let sql = format!("SELECT archive_watermark FROM {TASKS} WHERE id = $1 FOR UPDATE");
    let row = sqlx::query(&sql)
        .bind(task_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| integrity(&format!("Series task does not exist: {task_id}")))?;
    Ok(row.get("archive_watermark"))
}

async fn get_series_state_for_update_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    task_id: &str,
    series_id: &str,
) -> Result<Option<DurableSeriesState>, Box<dyn std::error::Error + Send + Sync>> {
    let sql =
        format!("SELECT * FROM {SERIES_STATE} WHERE task_id = $1 AND series_id = $2 FOR UPDATE");
    sqlx::query(&sql)
        .bind(task_id)
        .bind(series_id)
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(row_to_durable_series_state)
        .transpose()
}

async fn save_series_state_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &DurableSeriesState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sql = format!(
        r#"
        INSERT INTO {SERIES_STATE} (
            task_id, series_id, mode, event, through_index, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (task_id, series_id) DO UPDATE SET
            mode = EXCLUDED.mode,
            event = EXCLUDED.event,
            through_index = EXCLUDED.through_index,
            updated_at = EXCLUDED.updated_at
        WHERE {SERIES_STATE}.through_index < EXCLUDED.through_index
        "#
    );
    let mode = series_mode_to_string(&state.mode)
        .ok_or_else(|| integrity("Durable series mode is not serializable"))?;
    sqlx::query(&sql)
        .bind(&state.task_id)
        .bind(&state.series_id)
        .bind(mode)
        .bind(serde_json::to_value(&state.event)?)
        .bind(state.through_index as i64)
        .bind(now_millis())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn update_task_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    task: &Task,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sql = format!(
        r#"
        UPDATE {TASKS}
        SET status = $1, result = $2, error = $3, metadata = $4,
            updated_at = $5, completed_at = $6, tags = $7, assign_mode = $8,
            cost = $9, assigned_worker = $10, disconnect_policy = $11
        WHERE id = $12
        "#
    );
    let status = serde_json::to_value(&task.status)?
        .as_str()
        .unwrap_or("pending")
        .to_string();
    let assign_mode = task
        .assign_mode
        .as_ref()
        .map(enum_value_string)
        .transpose()?;
    let disconnect_policy = task
        .disconnect_policy
        .as_ref()
        .map(enum_value_string)
        .transpose()?;
    let result = task.result.as_ref().map(serde_json::to_value).transpose()?;
    let error = task.error.as_ref().map(serde_json::to_value).transpose()?;
    let metadata = task
        .metadata
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let tags = task.tags.as_ref().map(serde_json::to_value).transpose()?;
    sqlx::query(&sql)
        .bind(status)
        .bind(result)
        .bind(error)
        .bind(metadata)
        .bind(task.updated_at as i64)
        .bind(task.completed_at.map(|value| value as i64))
        .bind(tags)
        .bind(assign_mode)
        .bind(task.cost.map(|value| value as i32))
        .bind(&task.assigned_worker)
        .bind(disconnect_policy)
        .bind(&task.id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn upsert_canonical_event_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &TaskEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sql =
        format!("SELECT * FROM {EVENTS} WHERE id = $1 OR (task_id = $2 AND idx = $3) FOR UPDATE");
    let rows = sqlx::query(&sql)
        .bind(&event.id)
        .bind(&event.task_id)
        .bind(event.index as i32)
        .fetch_all(&mut **tx)
        .await?;
    if !rows.is_empty() {
        validate_canonical_event_rows(&rows, event)?;
        return Ok(());
    }
    if insert_canonical_event_pg_tx(tx, event).await? {
        return Ok(());
    }

    let raced_rows = sqlx::query(&sql)
        .bind(&event.id)
        .bind(&event.task_id)
        .bind(event.index as i32)
        .fetch_all(&mut **tx)
        .await?;
    validate_canonical_event_rows(&raced_rows, event)
}

async fn assert_compact_event_compatible_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &TaskEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sql =
        format!("SELECT * FROM {EVENTS} WHERE id = $1 OR (task_id = $2 AND idx = $3) FOR UPDATE");
    let rows = sqlx::query(&sql)
        .bind(&event.id)
        .bind(&event.task_id)
        .bind(event.index as i32)
        .fetch_all(&mut **tx)
        .await?;
    if rows.is_empty() {
        return Ok(());
    }
    if rows.len() != 1 {
        return Err(integrity(&format!(
            "Archive compact event identity conflicts at {}:{}",
            event.task_id, event.index
        )));
    }
    let existing = PostgresLongTermStore::row_to_event(&rows[0]);
    let field_matches = if event.series_mode == Some(SeriesMode::Accumulate) {
        existing.series_acc_field.as_deref().unwrap_or("delta")
            == event.series_acc_field.as_deref().unwrap_or("delta")
    } else {
        existing.series_acc_field == event.series_acc_field
    };
    if existing.task_id != event.task_id
        || existing.series_id != event.series_id
        || existing.series_mode != event.series_mode
        || !field_matches
    {
        return Err(integrity(&format!(
            "Archive compact event identity conflicts at {}:{}",
            event.task_id, event.index
        )));
    }
    Ok(())
}

fn validate_canonical_event_rows(
    rows: &[PgRow],
    event: &TaskEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if rows.len() != 1
        || archive_event_record(&PostgresLongTermStore::row_to_event(&rows[0]))?
            != archive_event_record(event)?
    {
        return Err(integrity(&format!(
            "Archive event identity conflicts at {}:{}",
            event.task_id, event.index
        )));
    }
    Ok(())
}

async fn insert_canonical_event_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &TaskEvent,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let sql = format!(
        r#"
        INSERT INTO {EVENTS} (
            id, task_id, idx, timestamp, type, level, data, series_id, series_mode, series_acc_field
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
        )
        ON CONFLICT DO NOTHING
        RETURNING id
        "#
    );
    let level = level_to_string(&event.level)?;
    let mode = event.series_mode.as_ref().and_then(series_mode_to_string);
    let inserted = sqlx::query(&sql)
        .bind(&event.id)
        .bind(&event.task_id)
        .bind(event.index as i32)
        .bind(event.timestamp as i64)
        .bind(&event.r#type)
        .bind(level)
        .bind(data_json_for_db(&event.data))
        .bind(&event.series_id)
        .bind(mode)
        .bind(&event.series_acc_field)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(inserted.is_some())
}

async fn install_canonical_series_event_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &DurableSeriesState,
    committed: Option<&DurableSeriesState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let event = &state.event;
    let select_sql = format!(
        "SELECT * FROM {EVENTS} \
         WHERE (task_id = $1 AND series_id = $2 \
                AND series_mode IN ('latest', 'accumulate')) \
            OR id = $3 OR (task_id = $1 AND idx = $4) \
         FOR UPDATE"
    );
    let rows = sqlx::query(&select_sql)
        .bind(&event.task_id)
        .bind(&state.series_id)
        .bind(&event.id)
        .bind(event.index as i32)
        .fetch_all(&mut **tx)
        .await?;
    let existing = rows
        .iter()
        .map(PostgresLongTermStore::row_to_event)
        .collect::<Vec<_>>();
    if existing.iter().any(|candidate| {
        candidate.task_id == event.task_id
            && candidate.series_id.as_deref() == Some(state.series_id.as_str())
            && (candidate.series_mode.as_ref() != Some(&state.mode)
                || candidate.series_acc_field != event.series_acc_field)
    }) {
        return Err(integrity(&format!(
            "Archive series semantics conflict for {}",
            state.series_id
        )));
    }

    let identity = existing
        .iter()
        .filter(|candidate| {
            candidate.id == event.id
                || (candidate.task_id == event.task_id && candidate.index == event.index)
        })
        .collect::<Vec<_>>();
    if identity.len() > 1 {
        return Err(integrity(&format!(
            "Archive event identity conflicts at {}:{}",
            event.task_id, event.index
        )));
    }
    if let Some(identity) = identity.first() {
        if archive_event_record(identity)? != archive_event_record(event)? {
            let can_advance_accumulation = state.mode == SeriesMode::Accumulate
                && identity.task_id == event.task_id
                && identity.id == event.id
                && identity.index == event.index
                && identity.series_id.as_deref() == Some(state.series_id.as_str())
                && identity.series_mode == Some(SeriesMode::Accumulate)
                && identity.series_acc_field == event.series_acc_field
                && state.through_index
                    > committed
                        .map(|value| value.through_index)
                        .unwrap_or(identity.index);
            if !can_advance_accumulation {
                return Err(integrity(&format!(
                    "Archive event identity conflicts at {}:{}",
                    event.task_id, event.index
                )));
            }
            update_stored_series_event_pg(tx, identity, event).await?;
        }
    } else {
        upsert_canonical_event_pg_tx(tx, event).await?;
    }

    let delete_sql = format!(
        "DELETE FROM {EVENTS} WHERE task_id = $1 AND series_id = $2 \
         AND series_mode IN ('latest', 'accumulate') AND id <> $3"
    );
    sqlx::query(&delete_sql)
        .bind(&event.task_id)
        .bind(&state.series_id)
        .bind(&event.id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn storage_state_to_string(
    state: &StorageState,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    enum_value_string(state)
}

fn enum_value_string<T: serde::Serialize>(
    value: &T,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    serde_json::to_value(value)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| integrity("Enum did not serialize as a string"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn fits_postgres_bigint(value: f64) -> bool {
    const JS_MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    value.is_finite()
        && value.fract() == 0.0
        && (-JS_MAX_SAFE_INTEGER..=JS_MAX_SAFE_INTEGER).contains(&value)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

async fn insert_event_pg_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &TaskEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sql = format!(
        r#"
        INSERT INTO {EVENTS} (
            id, task_id, idx, timestamp, type, level, data, series_id, series_mode, series_acc_field
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
        )
        ON CONFLICT (id) DO NOTHING
        "#
    );
    let level_str = level_to_string(&event.level)?;
    let series_mode_str = event.series_mode.as_ref().and_then(series_mode_to_string);
    let data_json = data_json_for_db(&event.data);

    sqlx::query(&sql)
        .bind(&event.id)
        .bind(&event.task_id)
        .bind(event.index as i32)
        .bind(event.timestamp as i64)
        .bind(&event.r#type)
        .bind(&level_str)
        .bind(&data_json)
        .bind(&event.series_id)
        .bind(&series_mode_str)
        .bind(&event.series_acc_field)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

async fn update_stored_series_event_pg(
    tx: &mut Transaction<'_, Postgres>,
    existing: &TaskEvent,
    event: &TaskEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sql = format!(
        r#"
        UPDATE {EVENTS}
        SET timestamp = $1,
            type = $2,
            level = $3,
            data = $4,
            series_id = $5,
            series_mode = $6,
            series_acc_field = $7
        WHERE id = $8
        "#
    );
    let level_str = level_to_string(&event.level)?;
    let series_mode_str = event.series_mode.as_ref().and_then(series_mode_to_string);
    let data_json = data_json_for_db(&event.data);

    sqlx::query(&sql)
        .bind(event.timestamp as i64)
        .bind(&event.r#type)
        .bind(&level_str)
        .bind(&data_json)
        .bind(&event.series_id)
        .bind(&series_mode_str)
        .bind(&event.series_acc_field)
        .bind(&existing.id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

fn accumulate_task_event(previous: &TaskEvent, current: TaskEvent, field: &str) -> TaskEvent {
    let previous_text = previous
        .data
        .as_object()
        .and_then(|data| data.get(field))
        .and_then(|value| value.as_str());
    let current_text = current
        .data
        .as_object()
        .and_then(|data| data.get(field))
        .and_then(|value| value.as_str());

    match (previous_text, current_text) {
        (Some(previous_text), Some(current_text)) => {
            let mut data = current.data.as_object().cloned().unwrap_or_default();
            data.insert(
                field.to_string(),
                serde_json::Value::String(format!("{previous_text}{current_text}")),
            );
            TaskEvent {
                data: serde_json::Value::Object(data),
                ..current
            }
        }
        _ => current,
    }
}

fn level_to_string(level: &Level) -> Result<String, serde_json::Error> {
    serde_json::to_value(level).map(|value| value.as_str().unwrap_or("info").to_string())
}

fn series_mode_to_string(mode: &SeriesMode) -> Option<String> {
    serde_json::to_value(mode)
        .ok()
        .and_then(|value| value.as_str().map(|value| value.to_string()))
}

fn data_json_for_db(data: &JsonValue) -> Option<JsonValue> {
    if data.is_null() {
        None
    } else {
        Some(data.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn observed_operation_invokes_future_once() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/postgres")
            .unwrap();
        let store = PostgresLongTermStore::new(pool);
        let calls = AtomicUsize::new(0);

        let result: Result<(), BoxError> = store
            .observed(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(Box::new(sqlx::Error::PoolClosed) as BoxError)
            })
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(result
            .unwrap_err()
            .downcast_ref::<DependencyUnavailableError>()
            .is_some());
    }

    #[test]
    fn compact_coverage_uses_utf8_series_order() {
        let make_event = |index, series_id: &str| TaskEvent {
            id: format!("event-{index}"),
            task_id: "task".to_string(),
            index,
            timestamp: index as f64,
            r#type: "delta".to_string(),
            level: Level::Info,
            data: JsonValue::Null,
            series_id: Some(series_id.to_string()),
            series_mode: Some(SeriesMode::Latest),
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        };
        let batch = ArchiveBatch {
            receipt: ArchiveBatchReceipt {
                task_id: "task".to_string(),
                generation: "generation".to_string(),
                ordinal: 0,
                previous_batch_digest: None,
                batch_digest: "0".repeat(64),
                entry_count: 2,
                first_index: Some(0),
                last_index: Some(1),
            },
            events: vec![make_event(0, "\u{10000}"), make_event(1, "\u{e000}")],
            series_latest: vec![],
        };

        let coverage = build_batch_series_coverage(&batch).unwrap();
        assert_eq!(
            coverage
                .iter()
                .map(|entry| entry.series_id.as_str())
                .collect::<Vec<_>>(),
            vec!["\u{e000}", "\u{10000}"]
        );
    }

    #[test]
    fn bigint_validation_matches_javascript_safe_integer_domain() {
        assert!(fits_postgres_bigint(9_007_199_254_740_991.0));
        assert!(fits_postgres_bigint(-9_007_199_254_740_991.0));
        assert!(!fits_postgres_bigint(9_007_199_254_740_992.0));
        assert!(!fits_postgres_bigint(9_223_372_036_854_775_808.0));
    }

    #[test]
    fn status_serializes_for_db() {
        let status = TaskStatus::Running;
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v.as_str().unwrap(), "running");
    }

    #[test]
    fn status_deserializes_from_db_string() {
        let status: TaskStatus =
            serde_json::from_value(JsonValue::String("completed".to_string())).unwrap();
        assert_eq!(status, TaskStatus::Completed);
    }

    #[test]
    fn level_serializes_for_db() {
        let level = Level::Warn;
        let v = serde_json::to_value(&level).unwrap();
        assert_eq!(v.as_str().unwrap(), "warn");
    }

    #[test]
    fn level_deserializes_from_db_string() {
        let level: Level = serde_json::from_value(JsonValue::String("error".to_string())).unwrap();
        assert_eq!(level, Level::Error);
    }

    #[test]
    fn series_mode_roundtrip_through_string() {
        let mode = SeriesMode::Accumulate;
        let v = serde_json::to_value(&mode).unwrap();
        let s = v.as_str().unwrap().to_string();
        let back: SeriesMode = serde_json::from_value(JsonValue::String(s)).unwrap();
        assert_eq!(back, SeriesMode::Accumulate);
    }

    #[test]
    fn task_params_to_json_value() {
        let mut params = HashMap::new();
        params.insert("url".to_string(), serde_json::json!("https://example.com"));
        params.insert("depth".to_string(), serde_json::json!(3));
        let v = serde_json::to_value(&params).unwrap();
        assert_eq!(v["url"], "https://example.com");
        assert_eq!(v["depth"], 3);
    }

    #[test]
    fn task_error_to_json_value() {
        let err = TaskError {
            code: Some("TIMEOUT".to_string()),
            message: "Request timed out".to_string(),
            details: None,
        };
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["code"], "TIMEOUT");
        assert_eq!(v["message"], "Request timed out");
    }

    #[test]
    fn optional_json_none_stays_none() {
        let params: Option<HashMap<String, JsonValue>> = None;
        let json: Option<JsonValue> = params
            .as_ref()
            .map(|p| serde_json::to_value(p).unwrap_or(JsonValue::Null));
        assert!(json.is_none());
    }

    #[test]
    fn timestamp_f64_to_i64_conversion() {
        let ts: f64 = 1700000000000.0;
        let as_i64 = ts as i64;
        assert_eq!(as_i64, 1700000000000_i64);
        let back = as_i64 as f64;
        assert!((back - ts).abs() < f64::EPSILON);
    }

    #[test]
    fn ttl_u64_to_i32_conversion() {
        let ttl: u64 = 3600;
        let as_i32 = ttl as i32;
        assert_eq!(as_i32, 3600);
        let back = as_i32 as u64;
        assert_eq!(back, ttl);
    }

    #[test]
    fn assign_mode_serializes_for_db() {
        let mode = AssignMode::Pull;
        let v = serde_json::to_value(&mode).unwrap();
        assert_eq!(v.as_str().unwrap(), "pull");
    }

    #[test]
    fn assign_mode_deserializes_from_db_string() {
        let mode: AssignMode =
            serde_json::from_value(JsonValue::String("ws-offer".to_string())).unwrap();
        assert_eq!(mode, AssignMode::WsOffer);
    }

    #[test]
    fn assign_mode_roundtrip_all_variants() {
        for mode in &[
            AssignMode::External,
            AssignMode::Pull,
            AssignMode::WsOffer,
            AssignMode::WsRace,
        ] {
            let v = serde_json::to_value(mode).unwrap();
            let s = v.as_str().unwrap().to_string();
            let back: AssignMode = serde_json::from_value(JsonValue::String(s)).unwrap();
            assert_eq!(&back, mode);
        }
    }

    #[test]
    fn disconnect_policy_serializes_for_db() {
        let policy = DisconnectPolicy::Reassign;
        let v = serde_json::to_value(&policy).unwrap();
        assert_eq!(v.as_str().unwrap(), "reassign");
    }

    #[test]
    fn disconnect_policy_deserializes_from_db_string() {
        let policy: DisconnectPolicy =
            serde_json::from_value(JsonValue::String("fail".to_string())).unwrap();
        assert_eq!(policy, DisconnectPolicy::Fail);
    }

    #[test]
    fn disconnect_policy_roundtrip_all_variants() {
        for policy in &[
            DisconnectPolicy::Reassign,
            DisconnectPolicy::Mark,
            DisconnectPolicy::Fail,
        ] {
            let v = serde_json::to_value(policy).unwrap();
            let s = v.as_str().unwrap().to_string();
            let back: DisconnectPolicy = serde_json::from_value(JsonValue::String(s)).unwrap();
            assert_eq!(&back, policy);
        }
    }

    #[test]
    fn worker_audit_action_serializes_for_db() {
        let action = WorkerAuditAction::TaskAssigned;
        let v = serde_json::to_value(&action).unwrap();
        assert_eq!(v.as_str().unwrap(), "task_assigned");
    }

    #[test]
    fn worker_audit_action_deserializes_from_db_string() {
        let action: WorkerAuditAction =
            serde_json::from_value(JsonValue::String("heartbeat_timeout".to_string())).unwrap();
        assert_eq!(action, WorkerAuditAction::HeartbeatTimeout);
    }

    #[test]
    fn worker_audit_action_roundtrip_all_variants() {
        let actions = vec![
            WorkerAuditAction::Connected,
            WorkerAuditAction::Disconnected,
            WorkerAuditAction::Updated,
            WorkerAuditAction::TaskAssigned,
            WorkerAuditAction::TaskDeclined,
            WorkerAuditAction::TaskReclaimed,
            WorkerAuditAction::Draining,
            WorkerAuditAction::HeartbeatTimeout,
            WorkerAuditAction::PullRequest,
        ];
        for action in &actions {
            let v = serde_json::to_value(action).unwrap();
            let s = v.as_str().unwrap().to_string();
            let back: WorkerAuditAction = serde_json::from_value(JsonValue::String(s)).unwrap();
            assert_eq!(&back, action);
        }
    }

    #[test]
    fn cost_u32_to_i32_conversion() {
        let cost: u32 = 42;
        let as_i32 = cost as i32;
        assert_eq!(as_i32, 42);
        let back = as_i32 as u32;
        assert_eq!(back, cost);
    }

    #[test]
    fn tags_to_json_value() {
        let tags = vec!["gpu".to_string(), "large-model".to_string()];
        let v = serde_json::to_value(&tags).unwrap();
        assert_eq!(v, serde_json::json!(["gpu", "large-model"]));
    }

    #[test]
    fn tags_from_json_value() {
        let v = serde_json::json!(["gpu", "large-model"]);
        let tags: Vec<String> = serde_json::from_value(v).unwrap();
        assert_eq!(tags, vec!["gpu".to_string(), "large-model".to_string()]);
    }

    #[test]
    fn optional_tags_none_stays_none() {
        let tags: Option<Vec<String>> = None;
        let json: Option<JsonValue> = tags
            .as_ref()
            .map(|t| serde_json::to_value(t).unwrap_or(JsonValue::Null));
        assert!(json.is_none());
    }

    #[test]
    fn worker_audit_event_data_to_json_value() {
        let mut data = HashMap::new();
        data.insert("reason".to_string(), serde_json::json!("timeout"));
        let v = serde_json::to_value(&data).unwrap();
        assert_eq!(v["reason"], "timeout");
    }
}
