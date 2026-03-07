use clap::Args;

use crate::client::TaskcastClient;
use crate::node_config::NodeConfigManager;

// ─── Args ─────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct LogsArgs {
    /// Task ID to stream events from
    pub task_id: String,
    /// Filter by event types (CSV, supports wildcards)
    #[arg(long)]
    pub types: Option<String>,
    /// Filter by levels (CSV)
    #[arg(long)]
    pub levels: Option<String>,
    /// Target node
    #[arg(long)]
    pub node: Option<String>,
}

#[derive(Args, Debug)]
pub struct TailArgs {
    /// Filter by event types (CSV, supports wildcards)
    #[arg(long)]
    pub types: Option<String>,
    /// Filter by levels (CSV)
    #[arg(long)]
    pub levels: Option<String>,
    /// Target node
    #[arg(long)]
    pub node: Option<String>,
}

// ─── Formatting ───────────────────────────────────────────────────────────────

pub fn format_event(
    event_type: &str,
    level: &str,
    timestamp: i64,
    data: &serde_json::Value,
    task_id: Option<&str>,
) -> String {
    let dt = chrono::DateTime::from_timestamp_millis(timestamp)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    let time = dt.format("%H:%M:%S");

    let task_prefix = match task_id {
        Some(id) if id.len() >= 7 => format!("{}..  ", &id[..7]),
        Some(id) => format!("{}..  ", id),
        None => String::new(),
    };

    if event_type == "taskcast:done" || event_type == "taskcast.done" {
        let reason = data
            .as_object()
            .and_then(|obj| obj.get("reason"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return format!("[{time}] {task_prefix}[DONE] {reason}");
    }

    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "null".to_string());
    format!(
        "[{time}] {task_prefix}{:<16} {:<5} {data_str}",
        event_type, level
    )
}

// ─── SSE Consumer ─────────────────────────────────────────────────────────────

pub async fn consume_sse(
    url: &str,
    token: Option<&str>,
    mut on_event: impl FnMut(serde_json::Value, &str),
    mut on_done: Option<&mut dyn FnMut()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let mut req = client.get(url).header("Accept", "text/event-stream");
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }

    let res = req.send().await?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status().as_u16()).into());
    }

    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    let mut current_event = String::new();
    let mut current_data = String::new();

    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Parse SSE format
        loop {
            if let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.starts_with("event:") {
                    current_event = line[6..].trim().to_string();
                } else if line.starts_with("data:") {
                    current_data = line[5..].trim().to_string();
                } else if line.is_empty() {
                    // Empty line = end of event
                    if !current_data.is_empty() {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&current_data)
                        {
                            on_event(parsed, &current_event);
                            if current_event == "taskcast.done" {
                                if let Some(ref mut done_fn) = on_done {
                                    done_fn();
                                }
                            }
                        }
                    }
                    current_event.clear();
                    current_data.clear();
                }
            } else {
                break;
            }
        }
    }

    Ok(())
}

// ─── Commands ─────────────────────────────────────────────────────────────────

pub async fn run_logs(args: LogsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".taskcast");
    let mgr = NodeConfigManager::new(config_dir);

    let node = match args.node {
        Some(name) => match mgr.get(&name) {
            Some(entry) => entry,
            None => {
                eprintln!("Node \"{name}\" not found");
                std::process::exit(1);
            }
        },
        None => mgr.get_current(),
    };

    let client = TaskcastClient::from_node(&node).await?;

    let mut params = Vec::new();
    if let Some(ref types) = args.types {
        params.push(format!("types={types}"));
    }
    if let Some(ref levels) = args.levels {
        params.push(format!("levels={levels}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let url = format!(
        "{}/tasks/{}/events{qs}",
        client.base_url(),
        args.task_id
    );

    consume_sse(
        &url,
        client.token(),
        |event, sse_event_name| {
            if sse_event_name == "taskcast.done" {
                let reason = event
                    .as_object()
                    .and_then(|obj| obj.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let ts = chrono::Utc::now().timestamp_millis();
                println!(
                    "{}",
                    format_event(
                        "taskcast.done",
                        "info",
                        ts,
                        &serde_json::json!({ "reason": reason }),
                        None,
                    )
                );
            } else if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                println!("{}", format_event(event_type, level, timestamp, &data, None));
            }
        },
        Some(&mut || {
            std::process::exit(0);
        }),
    )
    .await?;

    Ok(())
}

pub async fn run_tail(args: TailArgs) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".taskcast");
    let mgr = NodeConfigManager::new(config_dir);

    let node = match args.node {
        Some(name) => match mgr.get(&name) {
            Some(entry) => entry,
            None => {
                eprintln!("Node \"{name}\" not found");
                std::process::exit(1);
            }
        },
        None => mgr.get_current(),
    };

    let client = TaskcastClient::from_node(&node).await?;

    let mut params = Vec::new();
    if let Some(ref types) = args.types {
        params.push(format!("types={types}"));
    }
    if let Some(ref levels) = args.levels {
        params.push(format!("levels={levels}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let url = format!("{}/events{qs}", client.base_url());

    consume_sse(
        &url,
        client.token(),
        |event, sse_event_name| {
            if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                let task_id = event.get("taskId").and_then(|v| v.as_str());
                println!(
                    "{}",
                    format_event(event_type, level, timestamp, &data, task_id)
                );
            }
        },
        None,
    )
    .await?;

    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_regular_event() {
        let result = format_event(
            "llm.delta",
            "info",
            // 2026-03-07T14:30:02Z = 1772975402000ms
            1772975402000,
            &json!({"delta": "Hello "}),
            None,
        );
        // Should contain HH:MM:SS time format
        assert!(result.contains(':'), "should contain time separator");
        assert!(result.contains("llm.delta"), "should contain event type");
        assert!(result.contains("info"), "should contain level");
        assert!(
            result.contains(r#""delta":"Hello ""#),
            "should contain JSON data, got: {result}"
        );
    }

    #[test]
    fn format_done_event() {
        let result = format_event(
            "taskcast.done",
            "info",
            1772975403000,
            &json!({"reason": "completed"}),
            None,
        );
        assert!(result.contains("[DONE] completed"), "got: {result}");
        assert!(!result.contains("info "), "done events should not show level padding");
    }

    #[test]
    fn format_done_event_colon_variant() {
        let result = format_event(
            "taskcast:done",
            "info",
            1772975403000,
            &json!({"reason": "failed"}),
            None,
        );
        assert!(result.contains("[DONE] failed"), "got: {result}");
    }

    #[test]
    fn format_event_with_task_id() {
        let result = format_event(
            "agent.step",
            "info",
            1772975402000,
            &json!({"step": 3}),
            Some("01JXX1234567890ABCDEF"),
        );
        assert!(result.contains("01JXX12..  "), "got: {result}");
        assert!(result.contains("agent.step"), "got: {result}");
        assert!(result.contains(r#""step":3"#), "got: {result}");
    }

    #[test]
    fn format_done_event_missing_reason() {
        let result = format_event(
            "taskcast.done",
            "info",
            1772975403000,
            &json!({}),
            None,
        );
        assert!(result.contains("[DONE] unknown"), "got: {result}");
    }

    #[test]
    fn format_done_event_null_data() {
        let result = format_event(
            "taskcast.done",
            "info",
            1772975403000,
            &serde_json::Value::Null,
            None,
        );
        assert!(result.contains("[DONE] unknown"), "got: {result}");
    }

    #[test]
    fn format_event_pads_type() {
        let result = format_event("x", "warn", 0, &json!({}), None);
        // "x" should be padded to 16 characters
        assert!(result.contains("x               "), "type should be padded, got: {result}");
    }

    #[test]
    fn format_event_pads_level() {
        let result = format_event("llm.delta", "info", 0, &json!({}), None);
        // "info" should be padded to 5 characters
        assert!(result.contains("info "), "level should be padded, got: {result}");
    }

    #[test]
    fn format_event_null_data_regular() {
        let result = format_event("test.event", "info", 0, &serde_json::Value::Null, None);
        assert!(result.contains("null"), "got: {result}");
    }

    #[test]
    fn format_done_event_with_task_id() {
        let result = format_event(
            "taskcast.done",
            "info",
            1772975403000,
            &json!({"reason": "timeout"}),
            Some("01JYYZZZZ0000000000000"),
        );
        assert!(result.contains("01JYYZZ..  "), "got: {result}");
        assert!(result.contains("[DONE] timeout"), "got: {result}");
    }

    #[test]
    fn format_event_short_task_id() {
        let result = format_event("test", "info", 0, &json!({}), Some("abc"));
        assert!(result.contains("abc..  "), "got: {result}");
    }
}
