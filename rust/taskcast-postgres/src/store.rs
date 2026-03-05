use async_trait::async_trait;
use serde_json::Value as JsonValue;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

use taskcast_core::types::{
    AssignMode, CleanupConfig, DisconnectPolicy, EventQueryOptions, Level, LongTermStore,
    SeriesMode, Task, TaskAuthConfig, TaskError, TaskEvent, TaskStatus, WebhookConfig,
    WorkerAuditAction, WorkerAuditEvent,
};

const TASKS: &str = "taskcast_tasks";
const EVENTS: &str = "taskcast_events";
const WORKER_EVENTS: &str = "taskcast_worker_events";

/// PostgreSQL-backed long-term store for tasks and events.
///
/// Uses `sqlx::PgPool` for connection pooling and implements the
/// `LongTermStore` trait from `taskcast-core`.
pub struct PostgresLongTermStore {
    pool: PgPool,
}

impl PostgresLongTermStore {
    /// Create a new store with the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run migrations to create/update tables and indexes.
    ///
    /// Uses sqlx's built-in migration runner which reads `.sql` files from
    /// the `migrations/` directory and tracks applied migrations in a
    /// `_sqlx_migrations` table.
    pub async fn migrate(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
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

        let assign_mode: Option<AssignMode> = assign_mode_str
            .and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());
        let disconnect_policy: Option<DisconnectPolicy> = disconnect_policy_str
            .and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());

        Task {
            id: row.get("id"),
            r#type: row.get("type"),
            status,
            params: params.and_then(|v| serde_json::from_value(v).ok()),
            result: result.and_then(|v| serde_json::from_value(v).ok()),
            error: error.and_then(|v| serde_json::from_value::<TaskError>(v).ok()),
            metadata: metadata.and_then(|v| serde_json::from_value(v).ok()),
            auth_config: auth_config
                .and_then(|v| serde_json::from_value::<TaskAuthConfig>(v).ok()),
            webhooks: webhooks
                .and_then(|v| serde_json::from_value::<Vec<WebhookConfig>>(v).ok()),
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
        let action: WorkerAuditAction =
            serde_json::from_value(JsonValue::String(action_str)).unwrap_or(WorkerAuditAction::Connected);

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
        let series_mode: Option<SeriesMode> = series_mode_str
            .and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());

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
        }
    }
}

#[async_trait]
impl LongTermStore for PostgresLongTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let params_json: Option<JsonValue> =
            task.params.as_ref().map(|p| serde_json::to_value(p).unwrap_or(JsonValue::Null));
        let result_json: Option<JsonValue> =
            task.result.as_ref().map(|r| serde_json::to_value(r).unwrap_or(JsonValue::Null));
        let error_json: Option<JsonValue> =
            task.error.as_ref().map(|e| serde_json::to_value(e).unwrap_or(JsonValue::Null));
        let metadata_json: Option<JsonValue> =
            task.metadata.as_ref().map(|m| serde_json::to_value(m).unwrap_or(JsonValue::Null));
        let auth_config_json: Option<JsonValue> = task
            .auth_config
            .as_ref()
            .map(|a| serde_json::to_value(a).unwrap_or(JsonValue::Null));
        let webhooks_json: Option<JsonValue> = task
            .webhooks
            .as_ref()
            .map(|w| serde_json::to_value(w).unwrap_or(JsonValue::Null));
        let cleanup_json: Option<JsonValue> =
            task.cleanup.as_ref().map(|c| serde_json::to_value(c).unwrap_or(JsonValue::Null));

        let created_at = task.created_at as i64;
        let updated_at = task.updated_at as i64;
        let completed_at = task.completed_at.map(|v| v as i64);
        let ttl = task.ttl.map(|v| v as i32);

        let tags_json: Option<JsonValue> =
            task.tags.as_ref().map(|t| serde_json::to_value(t).unwrap_or(JsonValue::Null));
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
                tags, assign_mode, cost, assigned_worker, disconnect_policy
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19
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
            "#
        );

        let status_str =
            serde_json::to_value(&task.status).map(|v| v.as_str().unwrap_or("pending").to_string())?;

        sqlx::query(&sql)
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
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!("SELECT * FROM {TASKS} WHERE id = $1");

        let row = sqlx::query(&sql)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(Self::row_to_task))
    }

    async fn save_event(
        &self,
        event: TaskEvent,
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
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
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
                let anchor_sql =
                    format!("SELECT idx FROM {EVENTS} WHERE id = $1");
                let anchor_row = sqlx::query(&anchor_sql)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;
                let anchor_idx: i32 = anchor_row
                    .as_ref()
                    .map(|r| r.get("idx"))
                    .unwrap_or(-1);

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
                let sql = format!(
                    "SELECT * FROM {EVENTS} WHERE task_id = $1 ORDER BY idx ASC LIMIT $2"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(limit_val)
                    .fetch_all(&self.pool)
                    .await?
            }
        } else {
            let sql = format!(
                "SELECT * FROM {EVENTS} WHERE task_id = $1 ORDER BY idx ASC LIMIT $2"
            );
            sqlx::query(&sql)
                .bind(task_id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
        };

        Ok(rows.iter().map(Self::row_to_event).collect())
    }

    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    }

    async fn get_worker_events(
        &self,
        worker_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
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
                let anchor_sql = format!(
                    "SELECT timestamp FROM {WORKER_EVENTS} WHERE id = $1"
                );
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
        let level: Level =
            serde_json::from_value(JsonValue::String("error".to_string())).unwrap();
        assert_eq!(level, Level::Error);
    }

    #[test]
    fn series_mode_roundtrip_through_string() {
        let mode = SeriesMode::Accumulate;
        let v = serde_json::to_value(&mode).unwrap();
        let s = v.as_str().unwrap().to_string();
        let back: SeriesMode =
            serde_json::from_value(JsonValue::String(s)).unwrap();
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
        let json: Option<JsonValue> =
            params.as_ref().map(|p| serde_json::to_value(p).unwrap_or(JsonValue::Null));
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
        for mode in &[AssignMode::External, AssignMode::Pull, AssignMode::WsOffer, AssignMode::WsRace] {
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
        for policy in &[DisconnectPolicy::Reassign, DisconnectPolicy::Mark, DisconnectPolicy::Fail] {
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
        let json: Option<JsonValue> =
            tags.as_ref().map(|t| serde_json::to_value(t).unwrap_or(JsonValue::Null));
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
