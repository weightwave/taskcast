use async_trait::async_trait;
use sqlx::SqlitePool;

use taskcast_core::types::{
    EventQueryOptions, LongTermStore, Task, TaskEvent, WorkerAuditEvent,
};

use crate::row_helpers::{
    assign_mode_to_string, audit_action_to_string, disconnect_policy_to_string,
    json_value_to_string, level_to_string, row_to_event, row_to_task,
    row_to_worker_audit_event, series_mode_to_string, status_to_string, to_json_string,
};

pub struct SqliteLongTermStore {
    pool: SqlitePool,
}

impl SqliteLongTermStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl LongTermStore for SqliteLongTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let status_str = status_to_string(&task.status);
        let params_json = to_json_string(&task.params);
        let result_json = to_json_string(&task.result);
        let error_json = to_json_string(&task.error);
        let metadata_json = to_json_string(&task.metadata);
        let auth_config_json = to_json_string(&task.auth_config);
        let webhooks_json = to_json_string(&task.webhooks);
        let cleanup_json = to_json_string(&task.cleanup);
        let tags_json = to_json_string(&task.tags);
        let assign_mode_str: Option<String> = task.assign_mode.as_ref().map(assign_mode_to_string);
        let cost = task.cost.map(|v| v as i32);
        let disconnect_policy_str: Option<String> = task
            .disconnect_policy
            .as_ref()
            .map(disconnect_policy_to_string);

        let created_at = task.created_at as i64;
        let updated_at = task.updated_at as i64;
        let completed_at = task.completed_at.map(|v| v as i64);
        let ttl = task.ttl.map(|v| v as i32);

        sqlx::query(
            r#"
            INSERT INTO taskcast_tasks (
                id, type, status, params, result, error, metadata,
                auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
                tags, assign_mode, cost, assigned_worker, disconnect_policy
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19
            )
            ON CONFLICT (id) DO UPDATE SET
                status = excluded.status,
                result = excluded.result,
                error = excluded.error,
                metadata = excluded.metadata,
                updated_at = excluded.updated_at,
                completed_at = excluded.completed_at,
                cost = excluded.cost,
                assigned_worker = excluded.assigned_worker
            "#,
        )
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
        .bind(cost)
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
        let row = sqlx::query("SELECT * FROM taskcast_tasks WHERE id = ?1")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(row_to_task))
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let level_str = level_to_string(&event.level);
        let series_mode_str: Option<String> =
            event.series_mode.as_ref().and_then(series_mode_to_string);
        let idx = event.index as i32;
        let timestamp = event.timestamp as i64;
        let data_str = json_value_to_string(&event.data);

        sqlx::query(
            r#"
            INSERT INTO taskcast_events (
                id, task_id, idx, timestamp, type, level, data, series_id, series_mode, series_acc_field
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
            )
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(&event.id)
        .bind(&event.task_id)
        .bind(idx)
        .bind(timestamp)
        .bind(&event.r#type)
        .bind(&level_str)
        .bind(&data_str)
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

        let limit_val = limit.map(|l| l as i64).unwrap_or(i64::MAX);

        let rows = if let Some(since) = since {
            if let Some(ref id) = since.id {
                // since.id takes priority: look up anchor's idx, then fetch events after it.
                // COALESCE ensures that if the anchor is not found, we return all events (idx > -1).
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1
                      AND idx > COALESCE(
                          (SELECT idx FROM taskcast_events WHERE id = ?2),
                          -1
                      )
                    ORDER BY idx ASC
                    LIMIT ?3
                    "#,
                )
                .bind(task_id)
                .bind(id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else if let Some(index) = since.index {
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1 AND idx > ?2
                    ORDER BY idx ASC
                    LIMIT ?3
                    "#,
                )
                .bind(task_id)
                .bind(index as i32)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else if let Some(timestamp) = since.timestamp {
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1 AND timestamp > ?2
                    ORDER BY idx ASC
                    LIMIT ?3
                    "#,
                )
                .bind(task_id)
                .bind(timestamp as i64)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else {
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1
                    ORDER BY idx ASC
                    LIMIT ?2
                    "#,
                )
                .bind(task_id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            }
        } else {
            sqlx::query(
                r#"
                SELECT * FROM taskcast_events
                WHERE task_id = ?1
                ORDER BY idx ASC
                LIMIT ?2
                "#,
            )
            .bind(task_id)
            .bind(limit_val)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.iter().map(row_to_event).collect())
    }

    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let action_str = audit_action_to_string(&event.action);
        let timestamp = event.timestamp as i64;
        let data_json = to_json_string(&event.data);

        sqlx::query(
            r#"
            INSERT INTO taskcast_worker_events (
                id, worker_id, timestamp, action, data
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5
            )
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(&event.id)
        .bind(&event.worker_id)
        .bind(timestamp)
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
            if let Some(ref id) = since.id {
                // since.id takes priority: look up the anchor's timestamp, then fetch events after it.
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_worker_events
                    WHERE worker_id = ?1
                      AND timestamp > COALESCE(
                          (SELECT timestamp FROM taskcast_worker_events WHERE id = ?2),
                          -1
                      )
                    ORDER BY timestamp ASC
                    LIMIT ?3
                    "#,
                )
                .bind(worker_id)
                .bind(id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else if let Some(timestamp) = since.timestamp {
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_worker_events
                    WHERE worker_id = ?1 AND timestamp > ?2
                    ORDER BY timestamp ASC
                    LIMIT ?3
                    "#,
                )
                .bind(worker_id)
                .bind(timestamp as i64)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else {
                // since exists but has no usable cursor fields
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_worker_events
                    WHERE worker_id = ?1
                    ORDER BY timestamp ASC
                    LIMIT ?2
                    "#,
                )
                .bind(worker_id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            }
        } else {
            sqlx::query(
                r#"
                SELECT * FROM taskcast_worker_events
                WHERE worker_id = ?1
                ORDER BY timestamp ASC
                LIMIT ?2
                "#,
            )
            .bind(worker_id)
            .bind(limit_val)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.iter().map(row_to_worker_audit_event).collect())
    }
}
