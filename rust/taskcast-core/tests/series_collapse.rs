use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;
use taskcast_core::types::{Level, SeriesMode, TaskEvent};
use taskcast_core::series::collapse_accumulate_series;

fn make_event(id: &str, task_id: &str, index: u64, data: serde_json::Value) -> TaskEvent {
    TaskEvent {
        id: id.to_string(),
        task_id: task_id.to_string(),
        index,
        timestamp: 1000.0 + (index as f64) * 1000.0,
        r#type: "test".to_string(),
        level: Level::Info,
        data,
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

fn make_acc_event(
    id: &str,
    task_id: &str,
    index: u64,
    data: serde_json::Value,
    series_id: &str,
) -> TaskEvent {
    TaskEvent {
        series_id: Some(series_id.to_string()),
        series_mode: Some(SeriesMode::Accumulate),
        ..make_event(id, task_id, index, data)
    }
}

// ─── returns_unchanged_when_no_accumulate_series ─────────────────────────

#[tokio::test]
async fn returns_unchanged_when_no_accumulate_series() {
    let events = vec![
        make_event("e1", "task-1", 0, json!({ "text": "hello" })),
        make_event("e2", "task-1", 1, json!({ "text": "world" })),
    ];

    let called = Arc::new(AtomicBool::new(false));
    let called_clone = called.clone();
    let result = collapse_accumulate_series(&events, move |_task_id, _series_id| {
        called_clone.store(true, Ordering::SeqCst);
        async { Ok(None) }
    })
    .await
    .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result, events);
    assert!(!called.load(Ordering::SeqCst), "get_series_latest should not be called when no accumulate series");
}

// ─── collapses_accumulate_series_with_snapshot ───────────────────────────

#[tokio::test]
async fn collapses_accumulate_series_with_snapshot() {
    let events = vec![
        make_acc_event("e1", "task-1", 0, json!({ "delta": "A" }), "s1"),
        make_acc_event("e2", "task-1", 1, json!({ "delta": "B" }), "s1"),
        make_event("e3", "task-1", 2, json!({ "x": 1 })),
    ];

    let acc_snapshot = TaskEvent {
        data: json!({ "delta": "AB" }),
        ..make_acc_event("e2", "task-1", 1, json!(null), "s1")
    };

    let snapshot_clone = acc_snapshot.clone();
    let result = collapse_accumulate_series(&events, |_task_id, _series_id| {
        let snap = snapshot_clone.clone();
        async move { Ok(Some(snap)) }
    })
    .await
    .unwrap();

    assert_eq!(result.len(), 2);
    // First result: collapsed snapshot
    assert_eq!(result[0].series_snapshot, Some(true));
    assert_eq!(result[0].data, json!({ "delta": "AB" }));
    // Second result: non-series event preserved
    assert_eq!(result[1].id, "e3");
    assert_eq!(result[1].data, json!({ "x": 1 }));
}

// ─── falls_back_to_last_event_when_get_latest_returns_none ──────────────

#[tokio::test]
async fn falls_back_to_last_event_when_get_latest_returns_none() {
    let events = vec![
        make_acc_event("e1", "task-1", 0, json!({ "delta": "A" }), "s1"),
        make_acc_event("e2", "task-1", 1, json!({ "delta": "AB" }), "s1"),
    ];

    let result = collapse_accumulate_series(&events, |_task_id, _series_id| {
        async { Ok(None) }
    })
    .await
    .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].series_snapshot, Some(true));
    assert_eq!(result[0].id, "e2"); // last event in series used as fallback
    assert_eq!(result[0].data, json!({ "delta": "AB" }));
}

// ─── handles_multiple_accumulate_series_independently ────────────────────

#[tokio::test]
async fn handles_multiple_accumulate_series_independently() {
    let events = vec![
        make_acc_event("e1", "task-1", 0, json!({ "delta": "a" }), "s1"),
        make_acc_event("e2", "task-1", 1, json!({ "delta": "x" }), "s2"),
        make_acc_event("e3", "task-1", 2, json!({ "delta": "ab" }), "s1"),
        make_acc_event("e4", "task-1", 3, json!({ "delta": "xy" }), "s2"),
    ];

    let snap_s1 = TaskEvent {
        data: json!({ "delta": "S1-ACC" }),
        ..make_acc_event("snap-s1", "task-1", 0, json!(null), "s1")
    };
    let snap_s2 = TaskEvent {
        data: json!({ "delta": "S2-ACC" }),
        ..make_acc_event("snap-s2", "task-1", 0, json!(null), "s2")
    };

    let s1 = snap_s1.clone();
    let s2 = snap_s2.clone();
    let result = collapse_accumulate_series(&events, move |_task_id, series_id| {
        let s1 = s1.clone();
        let s2 = s2.clone();
        let sid = series_id.to_string();
        async move {
            if sid == "s1" {
                Ok(Some(s1))
            } else {
                Ok(Some(s2))
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].series_snapshot, Some(true));
    assert_eq!(result[1].series_snapshot, Some(true));
}

// ─── preserves_keep_all_and_latest_series_events ────────────────────────

#[tokio::test]
async fn preserves_keep_all_and_latest_series_events() {
    let ka_event = TaskEvent {
        series_id: Some("ka".to_string()),
        series_mode: Some(SeriesMode::KeepAll),
        ..make_event("e1", "task-1", 0, json!({ "text": "keep" }))
    };
    let lt_event = TaskEvent {
        series_id: Some("lt".to_string()),
        series_mode: Some(SeriesMode::Latest),
        ..make_event("e2", "task-1", 1, json!({ "text": "latest" }))
    };
    let acc1 = make_acc_event("e3", "task-1", 2, json!({ "delta": "a" }), "acc");
    let acc2 = make_acc_event("e4", "task-1", 3, json!({ "delta": "ab" }), "acc");

    let events = vec![ka_event, lt_event, acc1, acc2];

    let snapshot = TaskEvent {
        data: json!({ "text": "collapsed" }),
        ..make_acc_event("snap", "task-1", 0, json!(null), "acc")
    };

    let snap = snapshot.clone();
    let result = collapse_accumulate_series(&events, move |_task_id, _series_id| {
        let s = snap.clone();
        async move { Ok(Some(s)) }
    })
    .await
    .unwrap();

    assert_eq!(result.len(), 3);
    assert_eq!(result[0].id, "e1"); // keep-all preserved
    assert_eq!(result[1].id, "e2"); // latest preserved
    assert_eq!(result[2].series_snapshot, Some(true)); // accumulate collapsed
}

// ─── handles_empty_events ───────────────────────────────────────────────

#[tokio::test]
async fn handles_empty_events() {
    let result = collapse_accumulate_series(&[], |_task_id: &str, _series_id: &str| {
        async { Ok(None) }
    })
    .await
    .unwrap();

    assert!(result.is_empty());
}

// ─── propagates_error_from_get_series_latest ────────────────────────────

#[tokio::test]
async fn propagates_error_from_get_series_latest() {
    let events = vec![
        make_acc_event("e1", "task-1", 0, json!({ "delta": "A" }), "s1"),
    ];

    let result = collapse_accumulate_series(&events, |_task_id: &str, _series_id: &str| {
        async {
            Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "test error"))
                as Box<dyn std::error::Error + Send + Sync>)
        }
    })
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("test error"));
}

// ─── accumulate_event_with_no_series_id_falls_through ───────────────────

#[tokio::test]
async fn accumulate_event_with_no_series_id_falls_through() {
    // An event with series_mode=Accumulate but series_id=None should be
    // treated as a regular event and passed through unchanged.
    let orphan = TaskEvent {
        series_mode: Some(SeriesMode::Accumulate),
        series_id: None,
        ..make_event("orphan", "task-1", 0, json!({ "text": "no series id" }))
    };
    let acc1 = make_acc_event("e1", "task-1", 1, json!({ "delta": "A" }), "s1");
    let acc2 = make_acc_event("e2", "task-1", 2, json!({ "delta": "AB" }), "s1");

    let events = vec![orphan.clone(), acc1, acc2];

    let result = collapse_accumulate_series(&events, |_task_id: &str, _series_id: &str| {
        async { Ok(None) }
    })
    .await
    .unwrap();

    // orphan falls through to result.push, s1 series collapsed to snapshot
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].id, "orphan");
    assert!(result[0].series_snapshot.is_none());
    assert_eq!(result[1].series_snapshot, Some(true));
    assert_eq!(result[1].id, "e2"); // cold fallback uses last event
}
