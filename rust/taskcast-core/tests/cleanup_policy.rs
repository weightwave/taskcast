use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use taskcast_core::*;

fn config() -> ResolvedCleanupConfig {
    resolve_cleanup_config(&serde_json::from_value(json!({"cleanup": {"enabled": true, "rules": [
        {"match": {"taskTypes": ["search.*"], "status": ["completed", "cancelled"]}, "trigger": {"afterMs": 86400000}, "target": "events"},
        {"match": {"taskTypes": ["search.*"], "status": ["failed"]}, "trigger": {"afterMs": 604800000}, "target": "events"}
    ]}})).unwrap(), &HashMap::new()).unwrap()
}

fn task() -> Task {
    let mut task: Task = serde_json::from_value(json!({"id": "test", "type": "search.youtube", "status": "completed", "createdAt": 0, "updatedAt": 50000, "completedAt": 1000})).unwrap();
    let policy = resolve_task_cleanup_policy(Some("search.youtube"), None, &config(), 0.0)
        .unwrap()
        .unwrap();
    task.cleanup = Some(policy.cleanup);
    task.cleanup_policy_version = Some(policy.cleanup_policy_version);
    task.cleanup_resolved_at = Some(policy.cleanup_resolved_at);
    task
}

#[test]
fn defaults_off_and_validates_environment() {
    assert!(
        !resolve_cleanup_config(&TaskcastConfig::default(), &HashMap::new())
            .unwrap()
            .enabled
    );
    for (value, enabled) in [("true", true), ("false", false)] {
        assert_eq!(
            resolve_cleanup_config(
                &TaskcastConfig::default(),
                &HashMap::from([("TASKCAST_CLEANUP_ENABLED".into(), value.into())])
            )
            .unwrap()
            .enabled,
            enabled
        );
    }
    assert!(resolve_cleanup_config(
        &TaskcastConfig::default(),
        &HashMap::from([("TASKCAST_CLEANUP_ENABLED".into(), "yes".into())])
    )
    .is_err());
}

#[test]
fn uses_completion_only_for_enrolled_terminal_tasks() {
    assert_eq!(
        cleanup_deadline(&task(), CleanupTarget::Events),
        Some(86401000.0)
    );
    let mut t = task();
    t.status = TaskStatus::Failed;
    assert_eq!(
        cleanup_deadline(&t, CleanupTarget::Events),
        Some(604801000.0)
    );
    for status in [
        TaskStatus::Pending,
        TaskStatus::Assigned,
        TaskStatus::Running,
        TaskStatus::Paused,
        TaskStatus::Blocked,
        TaskStatus::Timeout,
    ] {
        t.status = status;
        assert_eq!(cleanup_deadline(&t, CleanupTarget::Events), None);
    }
    t = task();
    t.completed_at = None;
    assert_eq!(cleanup_deadline(&t, CleanupTarget::Events), None);
    t = task();
    t.cleanup_policy_version = None;
    assert_eq!(cleanup_deadline(&t, CleanupTarget::Events), None);
    for completed_at in [-1.0, f64::NAN, f64::INFINITY, 9007199254740991.0] {
        t = task();
        t.completed_at = Some(completed_at);
        assert_eq!(cleanup_deadline(&t, CleanupTarget::Events), None);
    }
}

#[test]
fn matches_types_and_replaces_the_whole_policy() {
    for ty in [None, Some("other")] {
        assert!(resolve_task_cleanup_policy(ty, None, &config(), 123.0)
            .unwrap()
            .unwrap()
            .cleanup
            .rules
            .is_empty());
    }
    let empty = CleanupConfig { rules: vec![] };
    let p = resolve_task_cleanup_policy(Some("search.youtube"), Some(&empty), &config(), 123.0)
        .unwrap()
        .unwrap();
    assert!(p.cleanup.rules.is_empty());
    assert_eq!(p.cleanup_resolved_at, 123.0);
    let all: CleanupConfig =
        serde_json::from_value(json!({"rules": [{"target": "all", "trigger": {}}]})).unwrap();
    assert_eq!(
        resolve_task_cleanup_policy(None, Some(&all), &config(), 0.0)
            .unwrap()
            .unwrap()
            .cleanup,
        all
    );
}

#[test]
fn earliest_rule_for_each_target_wins() {
    let mut t = task();
    t.cleanup = Some(
        serde_json::from_value(json!({"rules": [
            {"target": "events", "trigger": {"afterMs": 500}},
            {"target": "events", "trigger": {"afterMs": 800}},
            {"target": "all", "trigger": {"afterMs": 900}}
        ]}))
        .unwrap(),
    );
    assert_eq!(cleanup_deadline(&t, CleanupTarget::Events), Some(1500.0));
    assert_eq!(cleanup_deadline(&t, CleanupTarget::All), Some(1900.0));
}

#[test]
fn enabled_rejects_unsupported_rules_but_disabled_keeps_legacy_config() {
    let bad: Vec<Value> = vec![
        json!({"target": "task", "trigger": {}}),
        json!({"target": "events", "trigger": {}, "eventFilter": {}}),
        json!({"target": "events", "trigger": {"afterMs": -1}}),
        json!({"target": "events", "trigger": {"afterMs": 0.5}}),
        json!({"target": "events", "trigger": {"afterMs": 9007199254740992u64}}),
        json!({"target": "events", "trigger": {}, "match": {"status": ["typo"]}}),
    ];
    for rule in bad {
        let enabled =
            serde_json::from_value(json!({"cleanup": {"enabled": true, "rules": [rule]}})).unwrap();
        assert!(resolve_cleanup_config(&enabled, &HashMap::new()).is_err());
        let disabled = serde_json::from_value(json!({"cleanup": {"rules": [rule]}})).unwrap();
        assert!(resolve_cleanup_config(&disabled, &HashMap::new()).is_ok());
    }
}

#[tokio::test]
async fn enrolls_new_tasks_without_retroactively_adopting_existing_ones() {
    let store = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let legacy = TaskEngine::new(TaskEngineOptions {
        short_term_store: store.clone(),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: Some(durable.clone()),
        hooks: None,
    });
    legacy
        .create_task(CreateTaskInput {
            id: Some("legacy".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: store,
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: Some(durable.clone()),
        hooks: None,
    })
    .with_cleanup_config(config())
    .unwrap();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("search.youtube".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(task.cleanup_policy_version, Some(1));
    assert_eq!(task.cleanup.unwrap().rules.len(), 2);
    assert_eq!(
        engine
            .get_task("legacy")
            .await
            .unwrap()
            .unwrap()
            .cleanup_policy_version,
        None
    );
    let invalid: CleanupConfig =
        serde_json::from_value(json!({"rules": [{"target": "task", "trigger": {}}]})).unwrap();
    assert!(matches!(
        engine
            .create_task(CreateTaskInput {
                cleanup: Some(invalid),
                ..Default::default()
            })
            .await,
        Err(EngineError::InvalidInput(_))
    ));
}
