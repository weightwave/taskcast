use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

use crate::types::{
    DurableSeriesState, SeriesLatestEntry, SeriesMode, Task, TaskArchive, TaskArchiveRestoreData,
    TaskEvent,
};

pub const TASK_ARCHIVE_SCHEMA: &str = "taskcast.taskArchive";
pub const TASK_ARCHIVE_VERSION: u64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("{0}")]
    Invalid(String),
}

impl ArchiveError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

pub fn validate_task_archive(archive: &TaskArchive) -> Result<TaskArchive, ArchiveError> {
    if archive.schema != TASK_ARCHIVE_SCHEMA {
        return Err(ArchiveError::invalid(format!(
            "Unsupported archive schema: {}",
            archive.schema
        )));
    }
    if archive.version != TASK_ARCHIVE_VERSION {
        return Err(ArchiveError::invalid(format!(
            "Unsupported archive version: {}",
            archive.version
        )));
    }
    if !archive.exported_at.is_finite() {
        return Err(ArchiveError::invalid(
            "Archive exported_at must be a finite number",
        ));
    }

    validate_task(&archive.task)?;

    let mut events = archive.events.clone();
    events.sort_by_key(|event| event.index);

    let mut seen_ids = HashSet::new();
    let mut seen_indexes = HashSet::new();
    for (expected_index, event) in events.iter().enumerate() {
        validate_event_shape(event)?;

        if event.task_id != archive.task.id {
            return Err(ArchiveError::invalid(format!(
                "Archive event task_id mismatch for event {}",
                event.id
            )));
        }
        if !seen_ids.insert(event.id.clone()) {
            return Err(ArchiveError::invalid(format!(
                "Archive contains duplicate event id: {}",
                event.id
            )));
        }
        if !seen_indexes.insert(event.index) {
            return Err(ArchiveError::invalid(format!(
                "Archive contains duplicate event index: {}",
                event.index
            )));
        }
        if event.index != expected_index as u64 {
            return Err(ArchiveError::invalid(format!(
                "Archive events must have contiguous indexes from 0; expected {}, got {}",
                expected_index, event.index
            )));
        }
        validate_raw_archive_event(event)?;
    }

    Ok(TaskArchive {
        schema: archive.schema.clone(),
        version: archive.version,
        exported_at: archive.exported_at,
        task: archive.task.clone(),
        events: events
            .into_iter()
            .map(sanitize_task_archive_event)
            .collect(),
    })
}

pub fn build_task_archive_restore_data(
    archive: &TaskArchive,
) -> Result<TaskArchiveRestoreData, ArchiveError> {
    let normalized = validate_task_archive(archive)?;
    let next_index = normalized
        .events
        .iter()
        .map(|event| event.index)
        .max()
        .map_or(0, |index| index + 1);

    Ok(TaskArchiveRestoreData {
        task: normalized.task,
        events: normalized
            .events
            .iter()
            .cloned()
            .map(sanitize_task_archive_event)
            .collect(),
        next_index,
        series_latest: build_series_latest(&normalized.events),
    })
}

pub fn sanitize_task_archive_event(mut event: TaskEvent) -> TaskEvent {
    event.series_snapshot = None;
    event._accumulated_data = None;
    event
}

pub fn canonical_json(value: &serde_json::Value) -> Result<String, ArchiveError> {
    match value {
        serde_json::Value::Null => Ok("null".to_string()),
        serde_json::Value::Bool(value) => Ok(value.to_string()),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_f64() {
                canonical_f64(value)
            } else {
                Err(ArchiveError::invalid(
                    "Archive digest JSON number is not representable",
                ))
            }
        }
        serde_json::Value::String(value) => {
            serde_json::to_string(value).map_err(|error| ArchiveError::invalid(error.to_string()))
        }
        serde_json::Value::Array(values) => {
            let encoded = values
                .iter()
                .map(canonical_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("[{}]", encoded.join(",")))
        }
        serde_json::Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(left, _)| *left);
            let encoded = entries
                .into_iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key)
                        .map_err(|error| ArchiveError::invalid(error.to_string()))?;
                    Ok(format!("{key}:{}", canonical_json(value)?))
                })
                .collect::<Result<Vec<_>, ArchiveError>>()?;
            Ok(format!("{{{}}}", encoded.join(",")))
        }
    }
}

pub fn archive_event_record(event: &TaskEvent) -> Result<String, ArchiveError> {
    canonical_json(&serde_json::json!([
        "taskcast-event-v1",
        event.id,
        event.task_id,
        event.index.to_string(),
        canonical_f64(event.timestamp)?,
        event.r#type,
        enum_string(&event.level)?,
        event.data,
        event.series_id,
        event.series_mode.as_ref().map(enum_string).transpose()?,
        event.series_acc_field,
    ]))
}

pub fn durable_series_state_record(state: &DurableSeriesState) -> Result<String, ArchiveError> {
    canonical_json(&serde_json::json!([
        "taskcast-series-v1",
        state.task_id,
        state.series_id,
        enum_string(&state.mode)?,
        state.through_index.to_string(),
        archive_event_record(&state.event)?,
    ]))
}

pub fn compute_archive_batch_digest(
    previous_batch_digest: Option<&str>,
    events: &[TaskEvent],
    series_latest: &[DurableSeriesState],
) -> Result<String, ArchiveError> {
    let mut lines = vec![
        "taskcast-batch-v1".to_string(),
        previous_batch_digest.unwrap_or_default().to_string(),
    ];
    lines.extend(
        events
            .iter()
            .map(archive_event_record)
            .collect::<Result<Vec<_>, _>>()?,
    );
    lines.extend(sorted_series_records(series_latest)?);
    Ok(sha256_hex(&lines.join("\n")))
}

pub fn compute_archive_source_page_digest(events: &[TaskEvent]) -> Result<String, ArchiveError> {
    let mut lines = vec!["taskcast-source-page-v1".to_string()];
    lines.extend(
        events
            .iter()
            .map(|event| canonical_json(&serde_json::json!([event.index.to_string(), event.id])))
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(sha256_hex(&lines.join("\n")))
}

pub fn compute_archive_source_digest(page_digests: &[String]) -> String {
    let mut lines = vec!["taskcast-source-v1".to_string()];
    lines.extend_from_slice(page_digests);
    sha256_hex(&lines.join("\n"))
}

pub fn compute_series_state_digest(
    series_latest: &[DurableSeriesState],
) -> Result<String, ArchiveError> {
    let mut lines = vec!["taskcast-series-state-v1".to_string()];
    lines.extend(sorted_series_records(series_latest)?);
    Ok(sha256_hex(&lines.join("\n")))
}

fn sorted_series_records(
    series_latest: &[DurableSeriesState],
) -> Result<Vec<String>, ArchiveError> {
    let mut states = series_latest.iter().collect::<Vec<_>>();
    states.sort_by(|left, right| {
        left.task_id
            .cmp(&right.task_id)
            .then_with(|| left.series_id.cmp(&right.series_id))
    });
    states
        .into_iter()
        .map(durable_series_state_record)
        .collect()
}

fn canonical_f64(value: f64) -> Result<String, ArchiveError> {
    if !value.is_finite() {
        return Err(ArchiveError::invalid(
            "Archive digest numbers must be finite",
        ));
    }
    if value == 0.0 {
        return Ok("0".to_string());
    }
    let mut buffer = ryu_js::Buffer::new();
    Ok(buffer.format(value).to_string())
}

fn enum_string<T: serde::Serialize>(value: &T) -> Result<String, ArchiveError> {
    serde_json::to_value(value)
        .map_err(|error| ArchiveError::invalid(error.to_string()))?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| ArchiveError::invalid("Archive digest enum must serialize as a string"))
}

fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_task(task: &Task) -> Result<(), ArchiveError> {
    validate_non_empty("Archive task.id", &task.id)?;
    validate_finite("Archive task.created_at", task.created_at)?;
    validate_finite("Archive task.updated_at", task.updated_at)?;
    if let Some(completed_at) = task.completed_at {
        validate_finite("Archive task.completed_at", completed_at)?;
    }
    if let Some(resume_at) = task.resume_at {
        validate_finite("Archive task.resume_at", resume_at)?;
    }
    Ok(())
}

fn validate_event_shape(event: &TaskEvent) -> Result<(), ArchiveError> {
    validate_non_empty("Archive event.id", &event.id)?;
    validate_non_empty("Archive event.task_id", &event.task_id)?;
    validate_non_empty("Archive event.type", &event.r#type)?;
    validate_finite("Archive event.timestamp", event.timestamp)?;
    if let Some(series_id) = &event.series_id {
        validate_non_empty("Archive event.series_id", series_id)?;
    }
    if let Some(series_acc_field) = &event.series_acc_field {
        validate_non_empty("Archive event.series_acc_field", series_acc_field)?;
    }
    Ok(())
}

fn validate_raw_archive_event(event: &TaskEvent) -> Result<(), ArchiveError> {
    if event.series_snapshot.is_some() {
        return Err(ArchiveError::invalid(format!(
            "Archive events cannot include presentation fields; series_snapshot is a collapsed presentation field on event {}",
            event.id
        )));
    }
    if event._accumulated_data.is_some() {
        return Err(ArchiveError::invalid(format!(
            "Archive events cannot include transient broadcast fields; _accumulated_data is transient on event {}",
            event.id
        )));
    }
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), ArchiveError> {
    if value.is_empty() {
        return Err(ArchiveError::invalid(format!(
            "{label} must be a non-empty string"
        )));
    }
    Ok(())
}

fn validate_finite(label: &str, value: f64) -> Result<(), ArchiveError> {
    if !value.is_finite() {
        return Err(ArchiveError::invalid(format!(
            "{label} must be a finite number"
        )));
    }
    Ok(())
}

fn build_series_latest(events: &[TaskEvent]) -> Vec<SeriesLatestEntry> {
    let mut index_by_key = HashMap::<String, usize>::new();
    let mut latest = Vec::<SeriesLatestEntry>::new();

    for event in events {
        let (series_id, series_mode) = match (&event.series_id, &event.series_mode) {
            (Some(series_id), Some(series_mode)) => (series_id, series_mode),
            _ => continue,
        };

        if series_mode == &SeriesMode::KeepAll {
            continue;
        }

        let key = format!("{}:{series_id}", event.task_id);
        let next_event = if series_mode == &SeriesMode::Accumulate {
            match index_by_key.get(&key).copied() {
                Some(existing_index) => accumulate_event(
                    &latest[existing_index].event,
                    event,
                    event.series_acc_field.as_deref().unwrap_or("delta"),
                ),
                None => sanitize_task_archive_event(event.clone()),
            }
        } else {
            sanitize_task_archive_event(event.clone())
        };

        if let Some(existing_index) = index_by_key.get(&key).copied() {
            latest[existing_index].event = next_event;
        } else {
            index_by_key.insert(key, latest.len());
            latest.push(SeriesLatestEntry {
                task_id: event.task_id.clone(),
                series_id: series_id.clone(),
                event: next_event,
            });
        }
    }

    latest
}

fn accumulate_event(previous: &TaskEvent, current: &TaskEvent, field: &str) -> TaskEvent {
    let Some(previous_text) = previous
        .data
        .as_object()
        .and_then(|data| data.get(field))
        .and_then(|value| value.as_str())
    else {
        return sanitize_task_archive_event(current.clone());
    };

    let Some(current_text) = current
        .data
        .as_object()
        .and_then(|data| data.get(field))
        .and_then(|value| value.as_str())
    else {
        return sanitize_task_archive_event(current.clone());
    };

    let mut data = current.data.as_object().cloned().unwrap_or_default();
    data.insert(
        field.to_string(),
        serde_json::Value::String(format!("{previous_text}{current_text}")),
    );

    TaskEvent {
        data: serde_json::Value::Object(data),
        ..sanitize_task_archive_event(current.clone())
    }
}
