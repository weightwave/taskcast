use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use taskcast_core::{
    apply_canonical_history_query, merge_canonical_history, resolve_canonical_series_latest,
    DurableSeriesState, EventQueryOptions, HotWriteToken, Level, LongTermStore,
    MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore, ReleasePreconditions,
    SeriesMode, ShortTermStore, SinceCursor, StorageCoordinator, Task, TaskEngine,
    TaskEngineOptions, TaskEvent, TaskStatus, TaskStorageMetadata, WorkerAuditEvent,
};

struct PagingLongTermStore {
    rows: Vec<TaskEvent>,
    state: DurableSeriesState,
    page_reads: AtomicUsize,
}

#[async_trait::async_trait]
impl LongTermStore for PagingLongTermStore {
    fn supports_hot_cold_release(&self) -> bool {
        true
    }

    async fn save_task(&self, _task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_task(
        &self,
        _task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Some(task()))
    }

    async fn save_event(
        &self,
        _event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_events(
        &self,
        _task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.page_reads.fetch_add(1, Ordering::SeqCst);
        let mut rows = self.rows.clone();
        if let Some(index) = opts
            .as_ref()
            .and_then(|opts| opts.since.as_ref())
            .and_then(|since| since.index)
        {
            rows.retain(|event| event.index > index);
        }
        if let Some(limit) = opts.and_then(|opts| opts.limit) {
            rows.truncate(limit as usize);
        }
        Ok(rows)
    }

    async fn get_task_storage_metadata(
        &self,
        _task_id: &str,
    ) -> Result<Option<TaskStorageMetadata>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Some(TaskStorageMetadata {
            task_id: "task-1".to_string(),
            storage_state: taskcast_core::StorageState::Hot,
            storage_epoch: 1,
            active_release_generation: None,
            archive_watermark: -1,
            last_event_at: None,
            cold_at: None,
            execution_deadline_at: None,
            task_version: 0,
        }))
    }

    async fn get_durable_series_state(
        &self,
        _task_id: &str,
    ) -> Result<Vec<DurableSeriesState>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(vec![self.state.clone()])
    }

    async fn save_worker_event(
        &self,
        _event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_worker_events(
        &self,
        _worker_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }
}

fn task() -> Task {
    Task {
        id: "task-1".to_string(),
        status: TaskStatus::Running,
        created_at: 1_000.0,
        updated_at: 1_000.0,
        r#type: None,
        params: None,
        result: None,
        error: None,
        metadata: None,
        completed_at: None,
        ttl: None,
        auth_config: None,
        webhooks: None,
        cleanup: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
    }
}

fn event(index: u64) -> TaskEvent {
    TaskEvent {
        id: format!("event-{index}"),
        task_id: "task-1".to_string(),
        index,
        timestamp: 2_000.0 + index as f64,
        r#type: "message".to_string(),
        level: Level::Info,
        data: serde_json::json!({ "index": index }),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

async fn history_engine() -> (
    Arc<MemoryShortTermStore>,
    Arc<MemoryLongTermStore>,
    TaskEngine,
) {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(task()).await.unwrap();
    durable.save_task(task()).await.unwrap();
    for index in 0..10 {
        durable.save_event(event(index)).await.unwrap();
    }
    for index in 8..11 {
        hot.append_event("task-1", event(index)).await.unwrap();
    }
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    (hot, durable, engine)
}

#[test]
fn canonical_merge_rejects_conflicts_and_applies_queries_after_overlay() {
    let durable = (0..10).map(event).collect::<Vec<_>>();
    let hot = (8..11).map(event).collect::<Vec<_>>();
    let merged = merge_canonical_history(&durable, &hot, &[]).unwrap();
    assert_eq!(
        merged.iter().map(|event| event.index).collect::<Vec<_>>(),
        (0..11).collect::<Vec<_>>()
    );
    let queried = apply_canonical_history_query(
        &merged,
        Some(EventQueryOptions {
            since: Some(SinceCursor {
                id: Some("event-7".to_string()),
                index: Some(1),
                timestamp: Some(2_001.0),
            }),
            limit: Some(2),
        }),
    );
    assert_eq!(
        queried.iter().map(|event| event.index).collect::<Vec<_>>(),
        vec![8, 9]
    );

    let mut conflict = event(0);
    conflict.id = "other-id".to_string();
    assert!(merge_canonical_history(&[event(0)], &[conflict], &[]).is_err());
    let mut conflict = event(0);
    conflict.data = serde_json::json!({ "changed": true });
    assert!(merge_canonical_history(&[event(0)], &[conflict], &[]).is_err());
}

#[test]
fn durable_accumulated_state_covers_old_deltas_and_applies_the_tail() {
    let mut accumulated = event(2);
    accumulated.id = "acc-2".to_string();
    accumulated.series_id = Some("output".to_string());
    accumulated.series_mode = Some(SeriesMode::Accumulate);
    accumulated.series_acc_field = Some("delta".to_string());
    accumulated.data = serde_json::json!({ "delta": "ABC" });
    let state = DurableSeriesState {
        task_id: "task-1".to_string(),
        series_id: "output".to_string(),
        mode: SeriesMode::Accumulate,
        event: accumulated.clone(),
        through_index: 2,
    };
    let hot = ["A", "B", "C", "D"]
        .into_iter()
        .enumerate()
        .map(|(index, delta)| {
            let mut event = event(index as u64);
            event.id = format!("acc-{index}");
            event.series_id = Some("output".to_string());
            event.series_mode = Some(SeriesMode::Accumulate);
            event.series_acc_field = Some("delta".to_string());
            event.data = serde_json::json!({ "delta": delta });
            event
        })
        .collect::<Vec<_>>();

    let merged = merge_canonical_history(&[accumulated.clone()], &hot, &[state.clone()]).unwrap();
    assert_eq!(merged, vec![accumulated, hot[3].clone()]);
    let latest = resolve_canonical_series_latest(&state, &hot).unwrap();
    assert_eq!(latest.data, serde_json::json!({ "delta": "ABCD" }));
    assert_eq!(latest.index, 3);
}

#[tokio::test]
async fn engine_uses_durable_baseline_with_hot_tail_and_cold_reads_stay_cold() {
    let (hot, durable, engine) = history_engine().await;
    let events = engine.get_events("task-1", None).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.index).collect::<Vec<_>>(),
        (0..11).collect::<Vec<_>>()
    );
    let events = engine
        .get_events(
            "task-1",
            Some(EventQueryOptions {
                since: Some(SinceCursor {
                    id: None,
                    index: Some(7),
                    timestamp: None,
                }),
                limit: Some(2),
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        events.iter().map(|event| event.index).collect::<Vec<_>>(),
        vec![8, 9]
    );

    StorageCoordinator::new(hot.clone(), durable.clone())
        .release_task_storage(
            "task-1",
            ReleasePreconditions {
                expected_last_event_index: 10,
                inactive_since: 3_000.0,
            },
        )
        .await
        .unwrap();
    let cold = engine
        .get_events(
            "task-1",
            Some(EventQueryOptions {
                since: None,
                limit: Some(2),
            }),
        )
        .await
        .unwrap();
    assert_eq!(cold, vec![event(0), event(1)]);
    let presence = hot.get_task_storage_presence("task-1").await.unwrap();
    assert!(!presence.task);
    assert_eq!(presence.event_count, 0);
}

#[tokio::test]
async fn bounded_history_pages_past_a_compacted_durable_row() {
    let mut compacted = event(0);
    compacted.id = "acc-first".to_string();
    compacted.series_id = Some("output".to_string());
    compacted.series_mode = Some(SeriesMode::Accumulate);
    compacted.series_acc_field = Some("delta".to_string());
    compacted.data = serde_json::json!({ "delta": "ABC" });
    let mut snapshot = compacted.clone();
    snapshot.id = "acc-snapshot".to_string();
    snapshot.index = 100;
    let durable = Arc::new(PagingLongTermStore {
        rows: vec![compacted, event(1), event(2)],
        state: DurableSeriesState {
            task_id: "task-1".to_string(),
            series_id: "output".to_string(),
            mode: SeriesMode::Accumulate,
            event: snapshot,
            through_index: 100,
        },
        page_reads: AtomicUsize::new(0),
    });
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });

    let events = engine
        .get_events(
            "task-1",
            Some(EventQueryOptions {
                since: None,
                limit: Some(2),
            }),
        )
        .await
        .unwrap();
    assert_eq!(events, vec![event(1), event(2)]);
    assert_eq!(durable.page_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn accumulated_latest_is_identical_before_and_after_release() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(task()).await.unwrap();
    durable.save_task(task()).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    for (index, delta) in ["A", "B", "C"].into_iter().enumerate() {
        let mut raw = event(index as u64);
        raw.id = format!("delta-{index}");
        raw.r#type = "llm.delta".to_string();
        raw.series_id = Some("output".to_string());
        raw.series_mode = Some(SeriesMode::Accumulate);
        raw.series_acc_field = Some("delta".to_string());
        raw.data = serde_json::json!({ "delta": delta });
        let committed = hot
            .commit_event_fenced("task-1", raw, &token)
            .await
            .unwrap();
        if index < 2 {
            durable
                .accumulate_series("task-1", "output", committed.event, "delta")
                .await
                .unwrap();
        }
    }
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    let hot_latest = engine.get_series_latest("task-1", "output").await.unwrap();
    assert_eq!(
        hot_latest.as_ref().unwrap().data,
        serde_json::json!({ "delta": "ABC" })
    );
    engine
        .release_task_storage(
            "task-1",
            ReleasePreconditions {
                expected_last_event_index: 2,
                inactive_since: 3_000.0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        engine.get_series_latest("task-1", "output").await.unwrap(),
        hot_latest
    );
}
