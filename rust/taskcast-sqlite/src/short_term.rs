use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use taskcast_core::types::{
    EventQueryOptions, ShortTermStore, Task, TaskEvent, TaskFilter, TaskStatus, Worker,
    WorkerAssignment, WorkerFilter,
};

use crate::row_helpers::{
    assign_mode_to_string, assignment_status_to_string, connection_mode_to_string,
    disconnect_policy_to_string, json_value_to_string, level_to_string, row_to_event, row_to_task,
    row_to_worker, row_to_worker_assignment, series_mode_to_string, status_to_string, to_json_string,
    worker_status_to_string,
};

pub struct SqliteShortTermStore {
    pool: SqlitePool,
}

impl SqliteShortTermStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ShortTermStore for SqliteShortTermStore {
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

    async fn next_index(
        &self,
        task_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let row = sqlx::query(
            r#"
            INSERT INTO taskcast_index_counters (task_id, counter)
            VALUES (?1, 0)
            ON CONFLICT (task_id) DO UPDATE SET counter = taskcast_index_counters.counter + 1
            RETURNING counter
            "#,
        )
        .bind(task_id)
        .fetch_one(&self.pool)
        .await?;

        let counter: i32 = row.get("counter");
        Ok(counter as u64)
    }

    async fn append_event(
        &self,
        _task_id: &str,
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

        // When no limit is specified, use a very large value (effectively unlimited).
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
                // since exists but has no usable cursor fields
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

    async fn set_ttl(
        &self,
        _task_id: &str,
        _ttl_seconds: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // No-op for SQLite — TTL expiration is handled at the engine level,
        // not via database-level expiry like Redis.
        Ok(())
    }

    async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let row = sqlx::query(
            "SELECT event_json FROM taskcast_series_latest WHERE task_id = ?1 AND series_id = ?2",
        )
        .bind(task_id)
        .bind(series_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let json_str: String = row.get("event_json");
                let event: TaskEvent = serde_json::from_str(&json_str)?;
                Ok(Some(event))
            }
            None => Ok(None),
        }
    }

    async fn set_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event_json = serde_json::to_string(&event)?;

        sqlx::query(
            r#"
            INSERT INTO taskcast_series_latest (task_id, series_id, event_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT (task_id, series_id) DO UPDATE SET
                event_json = excluded.event_json
            "#,
        )
        .bind(task_id)
        .bind(series_id)
        .bind(&event_json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        // Atomic read-modify-write in a single IMMEDIATE transaction
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await?;

        let row = sqlx::query(
            "SELECT event_json FROM taskcast_series_latest WHERE task_id = ?1 AND series_id = ?2",
        )
        .bind(task_id)
        .bind(series_id)
        .fetch_optional(&mut *conn)
        .await?;

        let prev: Option<TaskEvent> = match row {
            Some(r) => {
                let json_str: String = r.get("event_json");
                Some(serde_json::from_str(&json_str)?)
            }
            None => None,
        };

        let accumulated = if let Some(prev) = prev {
            let should_concat = prev
                .data
                .as_object()
                .and_then(|po| po.get(field)?.as_str().map(|s| s.to_string()))
                .and_then(|prev_val| {
                    event
                        .data
                        .as_object()
                        .and_then(|no| no.get(field)?.as_str().map(|s| s.to_string()))
                        .map(|new_val| (prev_val, new_val))
                });

            if let Some((prev_val, new_val)) = should_concat {
                let mut new_data = event.data.as_object().cloned().unwrap_or_default();
                new_data.insert(
                    field.to_string(),
                    serde_json::Value::String(prev_val + &new_val),
                );
                TaskEvent {
                    data: serde_json::Value::Object(new_data),
                    ..event
                }
            } else {
                event
            }
        } else {
            event
        };

        let event_json = serde_json::to_string(&accumulated)?;
        sqlx::query(
            r#"
            INSERT INTO taskcast_series_latest (task_id, series_id, event_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT (task_id, series_id) DO UPDATE SET
                event_json = excluded.event_json
            "#,
        )
        .bind(task_id)
        .bind(series_id)
        .bind(&event_json)
        .execute(&mut *conn)
        .await?;

        sqlx::query("COMMIT").execute(&mut *conn).await?;
        Ok(accumulated)
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Get the previous series latest event
        let prev = self.get_series_latest(task_id, series_id).await?;

        if let Some(prev) = prev {
            // Update only content fields of the previous event in the events table,
            // preserving the original event's id and idx to maintain position ordering.
            let level_str = level_to_string(&event.level);
            let series_mode_str: Option<String> =
                event.series_mode.as_ref().and_then(series_mode_to_string);
            let data_str = json_value_to_string(&event.data);

            sqlx::query(
                r#"
                UPDATE taskcast_events SET
                    type = ?1,
                    level = ?2,
                    data = ?3,
                    series_id = ?4,
                    series_mode = ?5
                WHERE id = ?6
                "#,
            )
            .bind(&event.r#type)
            .bind(&level_str)
            .bind(&data_str)
            .bind(&event.series_id)
            .bind(&series_mode_str)
            .bind(&prev.id)
            .execute(&self.pool)
            .await?;
        } else {
            // No previous series event — append as a new event
            self.append_event(task_id, event.clone()).await?;
        }

        // Always update the series latest entry
        self.set_series_latest(task_id, series_id, event).await?;

        Ok(())
    }

    async fn list_tasks(
        &self,
        filter: TaskFilter,
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        // Build query dynamically based on filter.
        // SQLite doesn't support array parameters, so we build WHERE clauses with
        // comma-separated IN lists and apply tag matching in Rust post-fetch.
        let mut conditions: Vec<String> = Vec::new();

        if let Some(ref statuses) = filter.status {
            if !statuses.is_empty() {
                let placeholders: Vec<String> = statuses
                    .iter()
                    .map(|s| format!("'{}'", status_to_string(s)))
                    .collect();
                conditions.push(format!("status IN ({})", placeholders.join(",")));
            }
        }

        if let Some(ref types) = filter.types {
            if !types.is_empty() {
                let placeholders: Vec<String> =
                    types.iter().map(|t| format!("'{}'", t.replace('\'', "''"))).collect();
                conditions.push(format!("type IN ({})", placeholders.join(",")));
            }
        }

        if let Some(ref modes) = filter.assign_mode {
            if !modes.is_empty() {
                let placeholders: Vec<String> = modes
                    .iter()
                    .map(|m| format!("'{}'", assign_mode_to_string(m)))
                    .collect();
                conditions.push(format!("assign_mode IN ({})", placeholders.join(",")));
            }
        }

        if let Some(ref exclude) = filter.exclude_task_ids {
            if !exclude.is_empty() {
                let placeholders: Vec<String> =
                    exclude.iter().map(|id| format!("'{}'", id.replace('\'', "''"))).collect();
                conditions.push(format!("id NOT IN ({})", placeholders.join(",")));
            }
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let query_str = format!(
            "SELECT * FROM taskcast_tasks{}",
            where_clause
        );

        let rows = sqlx::query(&query_str)
            .fetch_all(&self.pool)
            .await?;

        let mut tasks: Vec<Task> = rows.iter().map(row_to_task).collect();

        // Apply tag filtering in Rust since it requires JSON parsing
        if let Some(ref tag_matcher) = filter.tags {
            tasks.retain(|t| {
                taskcast_core::worker_matching::matches_tag(t.tags.as_deref(), tag_matcher)
            });
        }

        // Apply limit AFTER tag filtering to ensure correct result count
        if let Some(limit) = filter.limit {
            tasks.truncate(limit as usize);
        }

        Ok(tasks)
    }

    async fn save_worker(
        &self,
        worker: Worker,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let status_str = worker_status_to_string(&worker.status);
        let match_rule_json = serde_json::to_string(&worker.match_rule)?;
        let connection_mode_str = connection_mode_to_string(&worker.connection_mode);
        let metadata_json = to_json_string(&worker.metadata);
        let connected_at = worker.connected_at as i64;
        let last_heartbeat_at = worker.last_heartbeat_at as i64;

        sqlx::query(
            r#"
            INSERT INTO taskcast_workers (
                id, status, match_rule, capacity, used_slots, weight,
                connection_mode, connected_at, last_heartbeat_at, metadata
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
            )
            ON CONFLICT (id) DO UPDATE SET
                status = excluded.status,
                match_rule = excluded.match_rule,
                capacity = excluded.capacity,
                used_slots = excluded.used_slots,
                weight = excluded.weight,
                connection_mode = excluded.connection_mode,
                last_heartbeat_at = excluded.last_heartbeat_at,
                metadata = excluded.metadata
            "#,
        )
        .bind(&worker.id)
        .bind(&status_str)
        .bind(&match_rule_json)
        .bind(worker.capacity as i32)
        .bind(worker.used_slots as i32)
        .bind(worker.weight as i32)
        .bind(&connection_mode_str)
        .bind(connected_at)
        .bind(last_heartbeat_at)
        .bind(&metadata_json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_worker(
        &self,
        worker_id: &str,
    ) -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        let row = sqlx::query("SELECT * FROM taskcast_workers WHERE id = ?1")
            .bind(worker_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(row_to_worker))
    }

    async fn list_workers(
        &self,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        let mut conditions: Vec<String> = Vec::new();

        if let Some(ref f) = filter {
            if let Some(ref statuses) = f.status {
                if !statuses.is_empty() {
                    let placeholders: Vec<String> = statuses
                        .iter()
                        .map(|s| format!("'{}'", worker_status_to_string(s)))
                        .collect();
                    conditions.push(format!("status IN ({})", placeholders.join(",")));
                }
            }
            if let Some(ref modes) = f.connection_mode {
                if !modes.is_empty() {
                    let placeholders: Vec<String> = modes
                        .iter()
                        .map(|m| format!("'{}'", connection_mode_to_string(m)))
                        .collect();
                    conditions.push(format!("connection_mode IN ({})", placeholders.join(",")));
                }
            }
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let query_str = format!("SELECT * FROM taskcast_workers{}", where_clause);

        let rows = sqlx::query(&query_str)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows.iter().map(row_to_worker).collect())
    }

    async fn delete_worker(
        &self,
        worker_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query("DELETE FROM taskcast_workers WHERE id = ?1")
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn claim_task(
        &self,
        task_id: &str,
        worker_id: &str,
        cost: u32,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Use a transaction for atomicity
        let mut tx = self.pool.begin().await?;

        // Check worker exists and has capacity
        let worker_row = sqlx::query(
            "SELECT capacity, used_slots FROM taskcast_workers WHERE id = ?1",
        )
        .bind(worker_id)
        .fetch_optional(&mut *tx)
        .await?;

        let (capacity, used_slots) = match worker_row {
            Some(row) => {
                let cap: i32 = row.get("capacity");
                let used: i32 = row.get("used_slots");
                (cap as u32, used as u32)
            }
            None => {
                tx.rollback().await?;
                return Ok(false);
            }
        };

        if used_slots + cost > capacity {
            tx.rollback().await?;
            return Ok(false);
        }

        // Check task exists and is in a claimable state (Pending or Assigned)
        let task_row = sqlx::query(
            "SELECT status FROM taskcast_tasks WHERE id = ?1",
        )
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;

        match task_row {
            Some(row) => {
                let status_str: String = row.get("status");
                let status: TaskStatus = serde_json::from_value(
                    serde_json::Value::String(status_str),
                )
                .unwrap_or(TaskStatus::Running);

                if status != TaskStatus::Pending && status != TaskStatus::Assigned {
                    tx.rollback().await?;
                    return Ok(false);
                }
            }
            None => {
                tx.rollback().await?;
                return Ok(false);
            }
        }

        // Update worker used_slots
        sqlx::query("UPDATE taskcast_workers SET used_slots = used_slots + ?1 WHERE id = ?2")
            .bind(cost as i32)
            .bind(worker_id)
            .execute(&mut *tx)
            .await?;

        // Update task: set status to assigned, set assigned_worker and cost
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let assigned_status = status_to_string(&TaskStatus::Assigned);
        sqlx::query(
            r#"
            UPDATE taskcast_tasks SET
                status = ?1,
                assigned_worker = ?2,
                cost = ?3,
                updated_at = ?4
            WHERE id = ?5
            "#,
        )
        .bind(&assigned_status)
        .bind(worker_id)
        .bind(cost as i32)
        .bind(now)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(true)
    }

    async fn add_assignment(
        &self,
        assignment: WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let status_str = assignment_status_to_string(&assignment.status);
        let assigned_at = assignment.assigned_at as i64;

        sqlx::query(
            r#"
            INSERT INTO taskcast_worker_assignments (
                task_id, worker_id, cost, assigned_at, status
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5
            )
            ON CONFLICT (task_id) DO UPDATE SET
                worker_id = excluded.worker_id,
                cost = excluded.cost,
                assigned_at = excluded.assigned_at,
                status = excluded.status
            "#,
        )
        .bind(&assignment.task_id)
        .bind(&assignment.worker_id)
        .bind(assignment.cost as i32)
        .bind(assigned_at)
        .bind(&status_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn remove_assignment(
        &self,
        task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query("DELETE FROM taskcast_worker_assignments WHERE task_id = ?1")
            .bind(task_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_worker_assignments(
        &self,
        worker_id: &str,
    ) -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query("SELECT * FROM taskcast_worker_assignments WHERE worker_id = ?1")
            .bind(worker_id)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows.iter().map(row_to_worker_assignment).collect())
    }

    async fn get_task_assignment(
        &self,
        task_id: &str,
    ) -> Result<Option<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        let row = sqlx::query("SELECT * FROM taskcast_worker_assignments WHERE task_id = ?1")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(row_to_worker_assignment))
    }
}
