use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use taskcast_core::{
    DurableSeriesState, EventQueryOptions, HotWriteToken, Level, LongTermStore,
    MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore, PublishEventInput,
    ReleasePreconditions, ShortTermStore, StorageCoordinator, Task, TaskEngine, TaskEngineOptions,
    TaskEvent, TaskStatus, TaskStorageMetadata, TaskStorageMetadataCas, WorkerAuditEvent,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};
use tokio::sync::Semaphore;

struct BlockingHistoryStore {
    inner: Arc<MemoryLongTermStore>,
    blocked: AtomicBool,
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

#[async_trait::async_trait]
impl LongTermStore for BlockingHistoryStore {
    fn supports_hot_cold_release(&self) -> bool {
        true
    }

    fn supports_series_compaction(&self) -> bool {
        true
    }

    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.save_task(task).await
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_task(task_id).await
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.save_event(event).await
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner
            .replace_last_series_event(task_id, series_id, event)
            .await
    }

    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        self.inner
            .accumulate_series(task_id, series_id, event, field)
            .await
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        if !self.blocked.swap(true, Ordering::SeqCst) {
            self.started.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        self.inner.get_events(task_id, opts).await
    }

    async fn get_task_storage_metadata(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskStorageMetadata>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_task_storage_metadata(task_id).await
    }

    async fn compare_and_set_task_storage_metadata(
        &self,
        update: TaskStorageMetadataCas,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner
            .compare_and_set_task_storage_metadata(update)
            .await
    }

    async fn get_last_event_index(
        &self,
        task_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_last_event_index(task_id).await
    }

    async fn get_recent_events(
        &self,
        task_id: &str,
        limit: u64,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_recent_events(task_id, limit).await
    }

    async fn get_durable_series_state(
        &self,
        task_id: &str,
    ) -> Result<Vec<DurableSeriesState>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_durable_series_state(task_id).await
    }

    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.save_worker_event(event).await
    }

    async fn get_worker_events(
        &self,
        worker_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_worker_events(worker_id, opts).await
    }
}

fn running_task() -> Task {
    Task {
        id: "cold-sse-race".to_string(),
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

fn running_event() -> TaskEvent {
    TaskEvent {
        id: "running-event".to_string(),
        task_id: "cold-sse-race".to_string(),
        index: 0,
        timestamp: 2_000.0,
        r#type: "taskcast:status".to_string(),
        level: Level::Info,
        data: serde_json::json!({ "status": "running" }),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

async fn serve_app(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn publish_during_cold_snapshot_is_delivered_exactly_once() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(running_task()).await.unwrap();
    durable.save_task(running_task()).await.unwrap();
    let committed = hot
        .commit_event_fenced(
            "cold-sse-race",
            running_event(),
            &HotWriteToken {
                task_id: "cold-sse-race".to_string(),
                storage_epoch: 1,
            },
        )
        .await
        .unwrap();
    durable.save_event(committed.event).await.unwrap();
    StorageCoordinator::new(hot.clone(), durable.clone())
        .release_task_storage(
            "cold-sse-race",
            ReleasePreconditions {
                expected_last_event_index: 0,
                inactive_since: 2_000.0,
            },
        )
        .await
        .unwrap();

    let started = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let blocking = Arc::new(BlockingHistoryStore {
        inner: durable,
        blocked: AtomicBool::new(false),
        started: started.clone(),
        release: release.clone(),
    });
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        long_term_store: Some(blocking),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    }));
    let (app, _) = create_app(
        engine.clone(),
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
    );
    let addr = serve_app(app).await;
    let response = reqwest::Client::new()
        .get(format!(
            "http://{addr}/tasks/cold-sse-race/events?includeStatus=true"
        ))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    started.acquire().await.unwrap().forget();

    let raced = engine
        .publish_event(
            "cold-sse-race",
            PublishEventInput {
                r#type: "race.event".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "value": 1 }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .transition_task("cold-sse-race", TaskStatus::Completed, None)
        .await
        .unwrap();
    release.add_permits(1);

    let body = tokio::time::timeout(std::time::Duration::from_secs(5), response.text())
        .await
        .expect("SSE stream timed out")
        .unwrap();
    assert_eq!(body.matches(&format!("id: {}", raced.id)).count(), 1);
    assert_eq!(body.matches("\"type\":\"race.event\"").count(), 1);
    assert_eq!(body.matches("event: taskcast.done").count(), 1);
}
