use std::collections::{BTreeMap, HashMap};

use crate::types::{
    DurableSeriesState, EventQueryOptions, SeriesMode, StorageIntegrityError, TaskEvent,
};

pub fn merge_canonical_history(
    durable_events: &[TaskEvent],
    hot_events: &[TaskEvent],
    durable_series_state: &[DurableSeriesState],
) -> Result<Vec<TaskEvent>, StorageIntegrityError> {
    let mut states = HashMap::new();
    for state in durable_series_state {
        validate_series_state(state)?;
        let key = series_key(&state.task_id, &state.series_id);
        if states.insert(key, state).is_some() {
            return Err(StorageIntegrityError::new(format!(
                "Duplicate durable series state for {}:{}",
                state.task_id, state.series_id
            )));
        }
    }

    let mut by_index = BTreeMap::<u64, TaskEvent>::new();
    let mut by_id = HashMap::<String, TaskEvent>::new();
    let mut add = |event: &TaskEvent| -> Result<(), StorageIntegrityError> {
        if let Some(indexed) = by_index.get(&event.index) {
            if indexed.id != event.id {
                return Err(StorageIntegrityError::new(format!(
                    "Canonical history index {} has conflicting event identities",
                    event.index
                )));
            }
            if !same_event(indexed, event) {
                return Err(StorageIntegrityError::new(format!(
                    "Canonical history event {} has conflicting content",
                    event.id
                )));
            }
            return Ok(());
        }
        if by_id.contains_key(&event.id) {
            return Err(StorageIntegrityError::new(format!(
                "Canonical history event {} has conflicting index or content",
                event.id
            )));
        }
        by_index.insert(event.index, event.clone());
        by_id.insert(event.id.clone(), event.clone());
        Ok(())
    };

    for event in durable_events {
        if matching_series_state(event, &states)?.is_none() {
            add(event)?;
        }
    }
    for state in durable_series_state {
        add(&state.event)?;
    }
    for event in hot_events {
        if matching_series_state(event, &states)?
            .is_some_and(|state| event.index <= state.through_index)
        {
            continue;
        }
        add(event)?;
    }

    Ok(by_index.into_values().collect())
}

pub fn apply_canonical_history_query(
    events: &[TaskEvent],
    opts: Option<EventQueryOptions>,
) -> Vec<TaskEvent> {
    let mut start = 0;
    let since = opts.as_ref().and_then(|opts| opts.since.as_ref());
    if let Some(id) = since.and_then(|since| since.id.as_ref()) {
        if let Some(position) = events.iter().position(|event| &event.id == id) {
            start = position + 1;
        }
    }

    let mut result = events[start..].to_vec();
    if since.and_then(|since| since.id.as_ref()).is_none() {
        if let Some(index) = since.and_then(|since| since.index) {
            result.retain(|event| event.index > index);
        } else if let Some(timestamp) = since.and_then(|since| since.timestamp) {
            result.retain(|event| event.timestamp > timestamp);
        }
    }
    if let Some(limit) = opts.and_then(|opts| opts.limit) {
        result.truncate(limit as usize);
    }
    result
}

pub fn resolve_canonical_series_latest(
    durable_state: &DurableSeriesState,
    hot_events: &[TaskEvent],
) -> Result<TaskEvent, StorageIntegrityError> {
    validate_series_state(durable_state)?;
    let mut tail = hot_events
        .iter()
        .filter(|event| {
            event.task_id == durable_state.task_id
                && event.series_id.as_deref() == Some(durable_state.series_id.as_str())
                && event.series_mode.as_ref() == Some(&durable_state.mode)
                && event.index > durable_state.through_index
        })
        .cloned()
        .collect::<Vec<_>>();
    tail.sort_by_key(|event| event.index);

    if durable_state.mode == SeriesMode::Latest {
        return Ok(tail
            .into_iter()
            .last()
            .unwrap_or_else(|| durable_state.event.clone()));
    }

    let field = durable_state
        .event
        .series_acc_field
        .as_deref()
        .unwrap_or("delta")
        .to_string();
    let mut accumulated = durable_state.event.clone();
    for event in tail {
        let previous = accumulated
            .data
            .as_object()
            .and_then(|data| data.get(&field))
            .and_then(serde_json::Value::as_str);
        let current = event
            .data
            .as_object()
            .and_then(|data| data.get(&field))
            .and_then(serde_json::Value::as_str);
        if let (Some(previous), Some(current), Some(mut data)) =
            (previous, current, event.data.as_object().cloned())
        {
            data.insert(
                field.clone(),
                serde_json::Value::String(format!("{previous}{current}")),
            );
            accumulated = TaskEvent {
                data: serde_json::Value::Object(data),
                ..event
            };
        } else {
            accumulated = event;
        }
    }
    Ok(accumulated)
}

fn matching_series_state<'a>(
    event: &TaskEvent,
    states: &'a HashMap<String, &'a DurableSeriesState>,
) -> Result<Option<&'a DurableSeriesState>, StorageIntegrityError> {
    let Some(series_id) = event.series_id.as_ref() else {
        return Ok(None);
    };
    if !matches!(
        event.series_mode,
        Some(SeriesMode::Latest | SeriesMode::Accumulate)
    ) {
        return Ok(None);
    }
    let Some(state) = states.get(&series_key(&event.task_id, series_id)).copied() else {
        return Ok(None);
    };
    if event.series_mode.as_ref() != Some(&state.mode) {
        return Err(StorageIntegrityError::new(format!(
            "Canonical history series mode conflicts for {}:{}",
            event.task_id, series_id
        )));
    }
    Ok(Some(state))
}

fn validate_series_state(state: &DurableSeriesState) -> Result<(), StorageIntegrityError> {
    if state.event.task_id != state.task_id
        || state.event.series_id.as_deref() != Some(state.series_id.as_str())
        || state.event.series_mode.as_ref() != Some(&state.mode)
        || state.event.index != state.through_index
    {
        return Err(StorageIntegrityError::new(format!(
            "Durable series state is inconsistent for {}:{}",
            state.task_id, state.series_id
        )));
    }
    Ok(())
}

fn same_event(left: &TaskEvent, right: &TaskEvent) -> bool {
    left.id == right.id
        && left.task_id == right.task_id
        && left.index == right.index
        && left.timestamp == right.timestamp
        && left.r#type == right.r#type
        && left.level == right.level
        && left.data == right.data
        && left.series_id == right.series_id
        && left.series_mode == right.series_mode
        && left.series_acc_field == right.series_acc_field
}

fn series_key(task_id: &str, series_id: &str) -> String {
    format!("{task_id}\0{series_id}")
}
