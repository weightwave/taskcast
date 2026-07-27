use taskcast_core::archive::{
    archive_event_record, canonical_json, compute_archive_batch_digest,
    compute_archive_source_digest, compute_archive_source_page_digest, compute_series_state_digest,
};
use taskcast_core::types::{DurableSeriesState, Level, SeriesMode, TaskEvent};

fn fixture_event() -> TaskEvent {
    TaskEvent {
        id: "evt-7".to_string(),
        task_id: "task-1".to_string(),
        index: 7,
        timestamp: 1_700_000_000_123.0,
        r#type: "llm.delta".to_string(),
        level: Level::Info,
        data: serde_json::json!({
            "z": [3, { "b": true, "a": null }],
            "a": "hello"
        }),
        series_id: Some("output".to_string()),
        series_mode: Some(SeriesMode::Accumulate),
        series_acc_field: Some("delta".to_string()),
        series_snapshot: None,
        _accumulated_data: None,
    }
}

#[test]
fn canonical_json_is_independent_of_object_insertion_order() {
    assert_eq!(
        canonical_json(&serde_json::json!({ "z": 1, "a": { "y": 2, "b": 3 } })).unwrap(),
        r#"{"a":{"b":3,"y":2},"z":1}"#
    );
    assert_eq!(
        canonical_json(&serde_json::json!([
            1e21,
            1e20,
            1e-7,
            1e-6,
            -0.0,
            u64::MAX,
            667082108456853.2_f64
        ]))
        .unwrap(),
        "[1e+21,100000000000000000000,1e-7,0.000001,0,18446744073709552000,667082108456853.2]"
    );
}

#[test]
fn digest_protocol_matches_typescript_fixture() {
    let event = fixture_event();
    let series = DurableSeriesState {
        task_id: "task-1".to_string(),
        series_id: "output".to_string(),
        mode: SeriesMode::Accumulate,
        event: event.clone(),
        through_index: 7,
    };

    assert_eq!(
        archive_event_record(&event).unwrap(),
        r#"["taskcast-event-v1","evt-7","task-1","7","1700000000123","llm.delta","info",{"a":"hello","z":[3,{"a":null,"b":true}]},"output","accumulate","delta"]"#
    );
    assert_eq!(
        compute_archive_batch_digest(None, &[event.clone()], &[series.clone()]).unwrap(),
        "fcaa595fb88f042f2e86decfa48dd46483f80bd7edb04d5c8b7a5876345003d8"
    );
    let page_digest = compute_archive_source_page_digest(&[event]).unwrap();
    assert_eq!(
        page_digest,
        "a494e9437592b3a58deb02a98e414ba87cd591079695bb2ebd4dd4c04d506fc8"
    );
    assert_eq!(
        compute_archive_source_digest(&[page_digest]),
        "d25d5e5dd7d8dba54b03d2bf56156593d5d8394ecbf0a65ce1e3eae8fe1c050a"
    );
    assert_eq!(
        compute_series_state_digest(&[series]).unwrap(),
        "5b58f61debb90051f9a92459b3eb98776cfa02241d7cd9c09fedfa863c70d750"
    );
}
