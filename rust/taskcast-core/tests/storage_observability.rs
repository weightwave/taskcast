use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore,
    PublishEventInput, ReleasePreconditions, TaskEngine, TaskEngineOptions, TaskStatus,
};

#[tokio::test]
async fn reports_release_history_and_rehydration_without_payloads() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    let observations = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&observations);
    engine.add_storage_lifecycle_listener(Arc::new(move |observation| {
        captured.lock().unwrap().push(observation.clone());
    }));
    engine.add_storage_lifecycle_listener(Arc::new(|_| {
        panic!("observer failure must be isolated");
    }));

    engine
        .create_task(CreateTaskInput {
            id: Some("observed-task".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("observed-task", TaskStatus::Running, None)
        .await
        .unwrap();
    let event = engine
        .publish_event(
            "observed-task",
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "secretPayload": "must-not-be-logged" }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .release_task_storage(
            "observed-task",
            ReleasePreconditions {
                expected_last_event_index: event.index as i64,
                inactive_since: event.timestamp,
            },
        )
        .await
        .unwrap();
    engine.get_events("observed-task", None).await.unwrap();
    engine
        .publish_event(
            "observed-task",
            PublishEventInput {
                r#type: "owner.reacquired".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "secretPayload": "still-must-not-be-logged" }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let observations = observations.lock().unwrap();
    assert!(observations.iter().any(|value| {
        value["event"] == "storage_release"
            && value["taskId"] == "observed-task"
            && value["outcome"] == "released"
            && value["sourceEventCount"] == event.index + 1
            && value["storageStateBefore"] == "hot"
            && value["storageStateAfter"] == "cold"
            && value["archiveWatermark"] == event.index
    }));
    assert!(observations.iter().any(|value| {
        value["event"] == "storage_history_read"
            && value["taskId"] == "observed-task"
            && value["outcome"] == "success"
            && value["source"] == "durable"
            && value["eventCount"] == event.index + 1
    }));
    assert!(observations.iter().any(|value| {
        value["event"] == "storage_rehydrate"
            && value["taskId"] == "observed-task"
            && value["outcome"] == "rehydrated"
            && value["replayEventCount"] == event.index + 1
            && value["archiveWatermark"] == event.index
            && value["storageStateBefore"] == "cold"
            && value["storageStateAfter"] == "hot"
    }));
    let encoded = serde_json::to_string(&*observations).unwrap();
    assert!(!encoded.contains("must-not-be-logged"));
    assert!(!encoded.contains("\"data\""));
}

#[tokio::test]
async fn reports_release_precondition_conflicts() {
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        long_term_store: Some(Arc::new(MemoryLongTermStore::new())),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    let observations = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&observations);
    engine.add_storage_lifecycle_listener(Arc::new(move |observation| {
        captured.lock().unwrap().push(observation.clone());
    }));
    engine
        .create_task(CreateTaskInput {
            id: Some("conflict-task".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .release_task_storage(
            "conflict-task",
            ReleasePreconditions {
                expected_last_event_index: 99,
                inactive_since: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as f64,
            },
        )
        .await
        .unwrap_err();

    assert!(observations.lock().unwrap().iter().any(|value| {
        value["event"] == "storage_release"
            && value["taskId"] == "conflict-task"
            && value["outcome"] == "failed"
            && value["errorCode"] == "storage_precondition_failed"
    }));
}
