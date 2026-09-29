use serde_json::json;
use taskcast_core::{
    MemoryShortTermStore, ShortTermStore, StorageIntegrityError, Task, TaskEvent,
    TerminalProjection, Worker, WorkerAssignment, WorkerStatus,
};

async fn assert_assignment_conflict_is_atomic(already_present: bool) {
    let store = MemoryShortTermStore::new();
    let task: Task = serde_json::from_value(json!({
        "id": "task-1", "status": "running", "createdAt": 1000, "updatedAt": 1000,
    }))
    .unwrap();
    let worker: Worker = serde_json::from_value(json!({
        "id": "worker-1", "status": "busy", "matchRule": {}, "capacity": 1,
        "usedSlots": 1, "weight": 1, "connectionMode": "pull",
        "connectedAt": 1000, "lastHeartbeatAt": 1000,
    }))
    .unwrap();
    let assignment: WorkerAssignment = serde_json::from_value(json!({
        "taskId": "task-1", "workerId": "worker-1", "cost": 1,
        "assignedAt": 1000, "status": "running",
    }))
    .unwrap();
    let first_event: TaskEvent = serde_json::from_value(json!({
        "id": "event-0", "taskId": "task-1", "index": 0, "timestamp": 1000,
        "type": "llm.delta", "level": "info", "data": { "delta": "hello" },
    }))
    .unwrap();
    let mut projection: TerminalProjection = serde_json::from_value(json!({
        "projectionId": "timeout-projection",
        "task": { "id": "task-1", "status": "timeout", "createdAt": 1000,
            "updatedAt": 2000, "completedAt": 2000 },
        "event": { "id": "event-1", "taskId": "task-1", "index": 1,
            "timestamp": 2000, "type": "taskcast:status", "level": "info",
            "data": { "status": "timeout" } },
        "assignment": assignment,
        "claimToken": "claim", "claimUntil": 100000,
    }))
    .unwrap();
    store.save_task(task.clone()).await.unwrap();
    store.save_worker(worker.clone()).await.unwrap();
    store.add_assignment(assignment.clone()).await.unwrap();
    store
        .append_event("task-1", first_event.clone())
        .await
        .unwrap();
    assert_eq!(store.next_index("task-1").await.unwrap(), 0);
    if already_present {
        store
            .append_event("task-1", projection.event.clone())
            .await
            .unwrap();
        assert_eq!(store.next_index("task-1").await.unwrap(), 1);
    }
    let lease = store
        .acquire_storage_lock("task-1", "owner", "release", 10_000)
        .await
        .unwrap()
        .unwrap();
    store.close_write_fence(&lease, 1).await.unwrap();
    let before_snapshot = store.get_task_mutation_snapshot("task-1").await.unwrap();
    let before_events = store.get_events("task-1", None).await.unwrap();
    let before_fence = store.get_write_fence("task-1").await.unwrap();
    let before_presence = store.get_task_storage_presence("task-1").await.unwrap();
    projection.assignment.as_mut().unwrap().cost = 2;

    let error = store
        .project_terminal_fenced(&projection, &lease, 1, 2)
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<StorageIntegrityError>().is_some());
    assert!(error
        .to_string()
        .contains("conflicts with the hot assignment"));
    assert_eq!(
        (
            store.get_task_mutation_snapshot("task-1").await.unwrap(),
            store.get_events("task-1", None).await.unwrap(),
            store.get_task_assignment("task-1").await.unwrap(),
            store.get_worker("worker-1").await.unwrap(),
            store.get_write_fence("task-1").await.unwrap(),
            store.get_task_storage_presence("task-1").await.unwrap(),
        ),
        (
            before_snapshot,
            before_events,
            Some(assignment.clone()),
            Some(worker),
            before_fence,
            before_presence,
        ),
    );

    projection.assignment = Some(assignment);
    let result = store
        .project_terminal_fenced(&projection, &lease, 1, 2)
        .await
        .unwrap();
    assert_eq!(result.projected, !already_present);
    assert_eq!(result.token.storage_epoch, 2);
    assert_eq!(
        store.get_task("task-1").await.unwrap(),
        Some(projection.task)
    );
    assert_eq!(
        store.get_events("task-1", None).await.unwrap(),
        vec![first_event, projection.event]
    );
    assert!(store.get_task_assignment("task-1").await.unwrap().is_none());
    let recovered_worker = store.get_worker("worker-1").await.unwrap().unwrap();
    assert_eq!(recovered_worker.used_slots, 0);
    assert_eq!(recovered_worker.status, WorkerStatus::Idle);
    assert_eq!(store.next_index("task-1").await.unwrap(), 2);
}

#[tokio::test]
async fn assignment_conflict_keeps_new_terminal_projection_atomic() {
    assert_assignment_conflict_is_atomic(false).await;
}

#[tokio::test]
async fn assignment_conflict_keeps_replayed_terminal_projection_atomic() {
    assert_assignment_conflict_is_atomic(true).await;
}
