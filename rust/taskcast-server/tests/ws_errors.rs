//! WebSocket error response path tests.
//!
//! These tests cover the Err(e) branches in worker_ws.rs that produce
//! REGISTER_ERROR, UPDATE_ERROR, CLAIM_ERROR, DECLINE_ERROR, and DRAIN_ERROR
//! error codes. Most of these branches only fire when the underlying
//! ShortTermStore returns a real error, which the MemoryShortTermStore never
//! does. We therefore use a FailingShortTermStore wrapper that can be toggled
//! to fail specific operations after initial setup succeeds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum_test::TestServer;
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions};
use taskcast_core::{
    BroadcastProvider, EventQueryOptions, MemoryBroadcastProvider, MemoryShortTermStore,
    ShortTermStore, Task, TaskEngine, TaskEngineOptions, TaskEvent, TaskFilter, Worker,
    WorkerAssignment, WorkerFilter,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

// ─── Failing Store ──────────────────────────────────────────────────────────

/// A ShortTermStore wrapper around MemoryShortTermStore that can be toggled
/// to return errors on worker and task operations. Allows initial setup
/// (register, create tasks) to succeed, then flipping a flag to cause
/// subsequent operations to return Err.
struct FailingShortTermStore {
    inner: MemoryShortTermStore,
    fail_get_worker: AtomicBool,
    fail_save_worker: AtomicBool,
    fail_get_task: AtomicBool,
    fail_get_task_assignment: AtomicBool,
}

impl FailingShortTermStore {
    fn new() -> Self {
        Self {
            inner: MemoryShortTermStore::new(),
            fail_get_worker: AtomicBool::new(false),
            fail_save_worker: AtomicBool::new(false),
            fail_get_task: AtomicBool::new(false),
            fail_get_task_assignment: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ShortTermStore for FailingShortTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.save_task(task).await
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        if self.fail_get_task.load(Ordering::SeqCst) {
            return Err("injected get_task failure".into());
        }
        self.inner.get_task(task_id).await
    }

    async fn append_event(
        &self,
        task_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.append_event(task_id, event).await
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_events(task_id, opts).await
    }

    async fn set_ttl(
        &self,
        task_id: &str,
        ttl_seconds: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.set_ttl(task_id, ttl_seconds).await
    }

    async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_series_latest(task_id, series_id).await
    }

    async fn set_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.set_series_latest(task_id, series_id, event).await
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

    async fn next_index(
        &self,
        task_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.next_index(task_id).await
    }

    async fn list_tasks(
        &self,
        filter: TaskFilter,
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.list_tasks(filter).await
    }

    async fn save_worker(
        &self,
        worker: Worker,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.fail_save_worker.load(Ordering::SeqCst) {
            return Err("injected save_worker failure".into());
        }
        self.inner.save_worker(worker).await
    }

    async fn get_worker(
        &self,
        worker_id: &str,
    ) -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        if self.fail_get_worker.load(Ordering::SeqCst) {
            return Err("injected get_worker failure".into());
        }
        self.inner.get_worker(worker_id).await
    }

    async fn list_workers(
        &self,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.list_workers(filter).await
    }

    async fn delete_worker(
        &self,
        worker_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.delete_worker(worker_id).await
    }

    async fn claim_task(
        &self,
        task_id: &str,
        worker_id: &str,
        cost: u32,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.claim_task(task_id, worker_id, cost).await
    }

    async fn add_assignment(
        &self,
        assignment: WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.add_assignment(assignment).await
    }

    async fn remove_assignment(
        &self,
        task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.remove_assignment(task_id).await
    }

    async fn get_worker_assignments(
        &self,
        worker_id: &str,
    ) -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_worker_assignments(worker_id).await
    }

    async fn get_task_assignment(
        &self,
        task_id: &str,
    ) -> Result<Option<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        if self.fail_get_task_assignment.load(Ordering::SeqCst) {
            return Err("injected get_task_assignment failure".into());
        }
        self.inner.get_task_assignment(task_id).await
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn make_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
        None,
        CorsConfig::default(),
    );
    let server = TestServer::builder().http_transport().build(app);
    (engine, manager, server)
}

/// Build a server backed by a FailingShortTermStore so we can inject errors.
fn make_failing_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, Arc<FailingShortTermStore>, TestServer) {
    let store = Arc::new(FailingShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
        None,
        CorsConfig::default(),
    );
    let server = TestServer::builder().http_transport().build(app);
    (engine, manager, store, server)
}

async fn ws_register(ws: &mut axum_test::TestWebSocket, worker_id: &str, capacity: u32) -> String {
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": capacity,
        "workerId": worker_id
    }))
    .await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "registered");
    resp["workerId"].as_str().unwrap().to_string()
}

// ─── REGISTER_ERROR ─────────────────────────────────────────────────────────
// Lines 284-292: register_worker returns Err when capacity is 0.

#[tokio::test]
async fn ws_register_with_zero_capacity_returns_register_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 0
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "REGISTER_ERROR");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("capacity"));

    ws.close().await;
}

// Lines 284-292: register_worker returns Err when save_worker fails in store.

#[tokio::test]
async fn ws_register_with_failing_store_returns_register_error() {
    let (_engine, _manager, store, server) = make_failing_ws_server();

    // Make save_worker fail before registration
    store.fail_save_worker.store(true, Ordering::SeqCst);

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "fail-reg-w1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "REGISTER_ERROR");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("injected save_worker failure"));

    ws.close().await;
}

// ─── UPDATE_ERROR ───────────────────────────────────────────────────────────
// Lines 310-317: update_worker returns Err when store.get_worker fails.

#[tokio::test]
async fn ws_update_with_failing_store_returns_update_error() {
    let (_engine, _manager, store, server) = make_failing_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register succeeds (store is not failing yet for get_worker)
    ws_register(&mut ws, "upd-err-w1", 5).await;

    // Now make get_worker fail
    store.fail_get_worker.store(true, Ordering::SeqCst);

    ws.send_json(&json!({
        "type": "update",
        "capacity": 10
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "UPDATE_ERROR");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("injected get_worker failure"));

    ws.close().await;
}

// ─── CLAIM_ERROR (Accept) ───────────────────────────────────────────────────
// Lines 353-361: claim_task returns Err when engine.get_task fails via store.

#[tokio::test]
async fn ws_accept_with_failing_store_returns_claim_error() {
    let (_engine, _manager, store, server) = make_failing_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "acc-err-w1", 5).await;

    // Now make get_task fail
    store.fail_get_task.store(true, Ordering::SeqCst);

    ws.send_json(&json!({
        "type": "accept",
        "taskId": "some-task"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "CLAIM_ERROR");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("injected get_task failure"));

    ws.close().await;
}

// ─── CLAIM_ERROR (Claim) ────────────────────────────────────────────────────
// Lines 399-407: claim_task returns Err when engine.get_task fails via store.

#[tokio::test]
async fn ws_claim_with_failing_store_returns_claim_error() {
    let (_engine, _manager, store, server) = make_failing_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "clm-err-w1", 5).await;

    // Now make get_task fail
    store.fail_get_task.store(true, Ordering::SeqCst);

    ws.send_json(&json!({
        "type": "claim",
        "taskId": "some-task"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "CLAIM_ERROR");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("injected get_task failure"));

    ws.close().await;
}

// ─── DECLINE_ERROR ──────────────────────────────────────────────────────────
// Lines 435-443: decline_task returns Err when store.get_task_assignment fails.

#[tokio::test]
async fn ws_decline_with_failing_store_returns_decline_error() {
    let (_engine, _manager, store, server) = make_failing_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "dec-err-w1", 5).await;

    // Make get_task_assignment fail
    store
        .fail_get_task_assignment
        .store(true, Ordering::SeqCst);

    ws.send_json(&json!({
        "type": "decline",
        "taskId": "some-task"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "DECLINE_ERROR");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("injected get_task_assignment failure"));

    ws.close().await;
}

// ─── DRAIN_ERROR ────────────────────────────────────────────────────────────
// Lines 465-472: update_worker (with Draining status) returns Err when
// store.get_worker fails.

#[tokio::test]
async fn ws_drain_with_failing_store_returns_drain_error() {
    let (_engine, _manager, store, server) = make_failing_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "drn-err-w1", 5).await;

    // Now make get_worker fail
    store.fail_get_worker.store(true, Ordering::SeqCst);

    ws.send_json(&json!({ "type": "drain" })).await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "DRAIN_ERROR");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("injected get_worker failure"));

    ws.close().await;
}
