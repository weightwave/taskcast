use std::collections::HashMap;
use std::sync::Arc;

use taskcast_core::{
    BlockedRequest, BroadcastProvider, CreateTaskInput, MemoryBroadcastProvider,
    MemoryShortTermStore, TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus,
    TransitionPayload,
};
use tokio::sync::Mutex;

// ─── Helper: collect broadcast events ────────────────────────────────────────

fn make_engine_with_capture() -> (TaskEngine, Arc<MemoryBroadcastProvider>, Arc<Mutex<Vec<TaskEvent>>>) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let events = Arc::new(Mutex::new(Vec::<TaskEvent>::new()));
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: store,
        broadcast: broadcast.clone(),
        long_term_store: None,
        hooks: None,
    });
    (engine, broadcast, events)
}

fn make_engine() -> TaskEngine {
    TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    })
}

/// Helper: create a task and move it to Running status
async fn create_running_task(engine: &TaskEngine, task_id: &str) -> () {
    engine
        .create_task(CreateTaskInput {
            id: Some(task_id.to_string()),
            ttl: Some(3600),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(task_id, TaskStatus::Running, None)
        .await
        .unwrap();
}

// ─── Test 1: Transitioning to paused sets reason ─────────────────────────────

#[tokio::test]
async fn transitioning_to_paused_sets_reason() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    let task = engine
        .transition_task(
            "t1",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("user requested pause".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    assert_eq!(task.status, TaskStatus::Paused);
    assert_eq!(task.reason, Some("user requested pause".to_string()));
}

// ─── Test 2: Transitioning to blocked sets reason, blocked_request, resume_at ─

#[tokio::test]
async fn transitioning_to_blocked_sets_reason_blocked_request_resume_at() {
    let engine = make_engine();
    create_running_task(&engine, "t2").await;

    let task = engine
        .transition_task(
            "t2",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("needs approval".to_string()),
                blocked_request: Some(BlockedRequest {
                    request_type: "approval".to_string(),
                    data: serde_json::json!({"approver": "admin"}),
                }),
                resume_after_ms: Some(60000.0),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    assert_eq!(task.status, TaskStatus::Blocked);
    assert_eq!(task.reason, Some("needs approval".to_string()));
    assert!(task.blocked_request.is_some());
    let br = task.blocked_request.unwrap();
    assert_eq!(br.request_type, "approval");
    assert!(task.resume_at.is_some());
    // resume_at should be approximately now + 60000
    let resume_at = task.resume_at.unwrap();
    assert!(resume_at > task.updated_at);
    assert!(resume_at <= task.updated_at + 61000.0);
}

// ─── Test 3: Transitioning from paused to running clears reason ──────────────

#[tokio::test]
async fn transitioning_from_paused_to_running_clears_reason() {
    let engine = make_engine();
    create_running_task(&engine, "t3").await;

    // Move to paused with reason
    engine
        .transition_task(
            "t3",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("paused for maintenance".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Move back to running
    let task = engine
        .transition_task("t3", TaskStatus::Running, None)
        .await
        .unwrap();

    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.reason, None);
}

// ─── Test 4: Transitioning from blocked to running clears all suspended fields ─

#[tokio::test]
async fn transitioning_from_blocked_to_running_clears_suspended_fields() {
    let engine = make_engine();
    create_running_task(&engine, "t4").await;

    // Move to blocked with all fields
    engine
        .transition_task(
            "t4",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("waiting for input".to_string()),
                blocked_request: Some(BlockedRequest {
                    request_type: "user_input".to_string(),
                    data: serde_json::json!({"prompt": "enter value"}),
                }),
                resume_after_ms: Some(30000.0),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Move back to running
    let task = engine
        .transition_task("t4", TaskStatus::Running, None)
        .await
        .unwrap();

    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.reason, None);
    assert_eq!(task.blocked_request, None);
    assert_eq!(task.resume_at, None);
}

// ─── Test 5: Blocked with blocked_request emits taskcast:blocked event ───────

#[tokio::test]
async fn blocked_with_blocked_request_emits_blocked_event() {
    let (engine, broadcast, events) = make_engine_with_capture();
    create_running_task(&engine, "t5").await;

    // Subscribe to capture events
    let events_clone = events.clone();
    let _unsub = broadcast
        .subscribe(
            "t5",
            Box::new(move |event| {
                let events_clone = events_clone.clone();
                tokio::spawn(async move {
                    events_clone.lock().await.push(event);
                });
            }),
        )
        .await;

    engine
        .transition_task(
            "t5",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("need approval".to_string()),
                blocked_request: Some(BlockedRequest {
                    request_type: "approval".to_string(),
                    data: serde_json::json!({"item": "deploy"}),
                }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Give async tasks a moment to settle
    tokio::task::yield_now().await;

    let captured = events.lock().await;
    // Should have: taskcast:status + taskcast:blocked
    let blocked_events: Vec<_> = captured
        .iter()
        .filter(|e| e.r#type == "taskcast:blocked")
        .collect();
    assert_eq!(blocked_events.len(), 1, "expected exactly one taskcast:blocked event");

    let blocked_event = &blocked_events[0];
    let data = blocked_event.data.as_object().unwrap();
    assert_eq!(
        data.get("reason").unwrap(),
        &serde_json::Value::String("need approval".to_string())
    );
    let request = data.get("request").unwrap().as_object().unwrap();
    assert_eq!(
        request.get("type").unwrap(),
        &serde_json::Value::String("approval".to_string())
    );
}

// ─── Test 6: blocked→running emits taskcast:resolved event ──────────────────

#[tokio::test]
async fn blocked_to_running_emits_resolved_event() {
    let (engine, broadcast, events) = make_engine_with_capture();
    create_running_task(&engine, "t6").await;

    // Move to blocked with blocked_request
    engine
        .transition_task(
            "t6",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                blocked_request: Some(BlockedRequest {
                    request_type: "approval".to_string(),
                    data: serde_json::json!({}),
                }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Subscribe to capture events for the resolve transition
    let events_clone = events.clone();
    let _unsub = broadcast
        .subscribe(
            "t6",
            Box::new(move |event| {
                let events_clone = events_clone.clone();
                tokio::spawn(async move {
                    events_clone.lock().await.push(event);
                });
            }),
        )
        .await;

    // Resolve: blocked → running with result
    let mut resolution = HashMap::new();
    resolution.insert("approved".to_string(), serde_json::json!(true));

    engine
        .transition_task(
            "t6",
            TaskStatus::Running,
            Some(TransitionPayload {
                result: Some(resolution),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    tokio::task::yield_now().await;

    let captured = events.lock().await;
    let resolved_events: Vec<_> = captured
        .iter()
        .filter(|e| e.r#type == "taskcast:resolved")
        .collect();
    assert_eq!(resolved_events.len(), 1, "expected exactly one taskcast:resolved event");

    let resolved_event = &resolved_events[0];
    let data = resolved_event.data.as_object().unwrap();
    let resolution = data.get("resolution").unwrap().as_object().unwrap();
    assert_eq!(
        resolution.get("approved").unwrap(),
        &serde_json::json!(true)
    );
}

// ─── Test 7: resume_after_ms computes correct resume_at timestamp ────────────

#[tokio::test]
async fn resume_after_ms_computes_correct_resume_at() {
    let engine = make_engine();
    create_running_task(&engine, "t7").await;

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;

    let task = engine
        .transition_task(
            "t7",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                resume_after_ms: Some(120000.0), // 2 minutes
                blocked_request: Some(BlockedRequest {
                    request_type: "timer".to_string(),
                    data: serde_json::json!({}),
                }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;

    let resume_at = task.resume_at.unwrap();
    // resume_at should be approximately now + 120000
    assert!(resume_at >= before + 120000.0);
    assert!(resume_at <= after + 120000.0);
}

// ─── Test 8: Non-suspended transitions don't affect reason/blocked_request ───

#[tokio::test]
async fn non_suspended_transitions_clear_suspended_fields() {
    let engine = make_engine();

    // Create a task, move to running, then to paused with reason
    engine
        .create_task(CreateTaskInput {
            id: Some("t8".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .transition_task("t8", TaskStatus::Running, None)
        .await
        .unwrap();

    engine
        .transition_task(
            "t8",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("paused".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Move back to running (non-suspended) should clear reason
    let task = engine
        .transition_task("t8", TaskStatus::Running, None)
        .await
        .unwrap();

    assert_eq!(task.reason, None);
    assert_eq!(task.blocked_request, None);
    assert_eq!(task.resume_at, None);

    // Now complete the task -- reason/blocked_request should remain None
    let task = engine
        .transition_task("t8", TaskStatus::Completed, None)
        .await
        .unwrap();

    assert_eq!(task.reason, None);
    assert_eq!(task.blocked_request, None);
    assert_eq!(task.resume_at, None);
}

// ─── Test 9: Blocked without blocked_request does NOT emit taskcast:blocked ──

#[tokio::test]
async fn blocked_without_blocked_request_does_not_emit_blocked_event() {
    let (engine, broadcast, events) = make_engine_with_capture();
    create_running_task(&engine, "t9").await;

    let events_clone = events.clone();
    let _unsub = broadcast
        .subscribe(
            "t9",
            Box::new(move |event| {
                let events_clone = events_clone.clone();
                tokio::spawn(async move {
                    events_clone.lock().await.push(event);
                });
            }),
        )
        .await;

    // Transition to blocked WITHOUT blockedRequest
    engine
        .transition_task(
            "t9",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("just blocked".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    tokio::task::yield_now().await;

    let captured = events.lock().await;
    let blocked_events: Vec<_> = captured
        .iter()
        .filter(|e| e.r#type == "taskcast:blocked")
        .collect();
    assert_eq!(blocked_events.len(), 0, "should not emit taskcast:blocked without blockedRequest");
}

// ─── Test 10: blocked→running without prior blocked_request does NOT emit resolved ─

#[tokio::test]
async fn blocked_to_running_without_blocked_request_does_not_emit_resolved() {
    let (engine, broadcast, events) = make_engine_with_capture();
    create_running_task(&engine, "t10").await;

    // Block without blocked_request
    engine
        .transition_task(
            "t10",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("blocked".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let events_clone = events.clone();
    let _unsub = broadcast
        .subscribe(
            "t10",
            Box::new(move |event| {
                let events_clone = events_clone.clone();
                tokio::spawn(async move {
                    events_clone.lock().await.push(event);
                });
            }),
        )
        .await;

    // Unblock
    engine
        .transition_task("t10", TaskStatus::Running, None)
        .await
        .unwrap();

    tokio::task::yield_now().await;

    let captured = events.lock().await;
    let resolved_events: Vec<_> = captured
        .iter()
        .filter(|e| e.r#type == "taskcast:resolved")
        .collect();
    assert_eq!(resolved_events.len(), 0, "should not emit taskcast:resolved without prior blocked_request");
}

// ─── Test 11: Paused sets reason without blocked_request or resume_at ────────

#[tokio::test]
async fn paused_does_not_set_blocked_request_or_resume_at() {
    let engine = make_engine();
    create_running_task(&engine, "t11").await;

    let task = engine
        .transition_task(
            "t11",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("manual pause".to_string()),
                // Even if these are passed, paused shouldn't set them
                // (blocked_request and resume_after_ms are blocked-specific)
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    assert_eq!(task.status, TaskStatus::Paused);
    assert_eq!(task.reason, Some("manual pause".to_string()));
    assert_eq!(task.blocked_request, None);
    assert_eq!(task.resume_at, None);
}
