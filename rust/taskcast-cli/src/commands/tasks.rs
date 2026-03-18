use clap::{Args, Subcommand};

use crate::client::TaskcastClient;
use crate::node_config::NodeConfigManager;

#[derive(Args, Debug)]
pub struct TasksArgs {
    #[command(subcommand)]
    pub command: TasksCommands,
}

#[derive(Subcommand, Debug)]
pub enum TasksCommands {
    /// List tasks
    List {
        /// Filter by status (e.g. running)
        #[arg(long)]
        status: Option<String>,
        /// Filter by task type (e.g. llm.*)
        #[arg(long = "type")]
        task_type: Option<String>,
        /// Maximum number of tasks to show
        #[arg(long, default_value = "20")]
        limit: u32,
        /// Named node to query
        #[arg(long)]
        node: Option<String>,
    },
    /// Inspect a task and its recent events
    Inspect {
        /// Task ID to inspect
        task_id: String,
        /// Named node to query
        #[arg(long)]
        node: Option<String>,
    },
}

#[derive(serde::Deserialize, Debug)]
pub struct TaskListItem {
    pub id: String,
    #[serde(rename = "type")]
    pub task_type: Option<String>,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: Option<f64>,
}

#[derive(serde::Deserialize, Debug)]
struct ListTasksResponse {
    tasks: Vec<TaskListItem>,
}

#[derive(serde::Deserialize, Debug)]
pub struct TaskDetail {
    pub id: String,
    #[serde(rename = "type")]
    pub task_type: Option<String>,
    pub status: String,
    pub params: Option<serde_json::Value>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<f64>,
}

#[derive(serde::Deserialize, Debug)]
pub struct EventItem {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub level: Option<String>,
    #[serde(rename = "seriesId")]
    pub series_id: Option<String>,
    pub timestamp: Option<f64>,
}

pub fn format_task_list(tasks: &[TaskListItem]) -> String {
    if tasks.is_empty() {
        return "No tasks found.".to_string();
    }

    let header = format!(
        "{:<28}{:<13}{:<11}CREATED",
        "ID", "TYPE", "STATUS"
    );

    let mut lines = vec![header];
    for t in tasks {
        let id = t.id.as_str();
        let task_type = t.task_type.as_deref().unwrap_or("");
        let status = t.status.as_str();
        let created = format_timestamp(t.created_at);
        lines.push(format!(
            "{:<28}{:<13}{:<11}{}",
            id, task_type, status, created
        ));
    }

    lines.join("\n")
}

pub fn format_task_inspect(task: &TaskDetail, events: &[EventItem]) -> String {
    let mut lines = Vec::new();

    lines.push(format!("Task: {}", task.id));
    lines.push(format!(
        "  Type:    {}",
        task.task_type.as_deref().unwrap_or("")
    ));
    lines.push(format!("  Status:  {}", task.status));
    let params_str = match &task.params {
        Some(v) if !v.is_null() => serde_json::to_string(v).unwrap_or_default(),
        _ => String::new(),
    };
    lines.push(format!("  Params:  {}", params_str));
    lines.push(format!(
        "  Created: {}",
        format_timestamp(task.created_at)
    ));

    if events.is_empty() {
        lines.push(String::new());
        lines.push("No events.".to_string());
    } else {
        let last5: Vec<&EventItem> = events.iter().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect();
        lines.push(String::new());
        lines.push(format!("Recent Events (last {}):", last5.len()));
        for (i, e) in last5.iter().enumerate() {
            let event_type = e.event_type.as_deref().unwrap_or("");
            let level = e.level.as_deref().unwrap_or("");
            let series = match &e.series_id {
                Some(s) => format!("series:{}", s),
                None => String::new(),
            };
            let ts = format_timestamp(e.timestamp);
            lines.push(format!(
                "  #{:<2} {:<13}{:<7}{:<17}{}",
                i, event_type, level, series, ts
            ));
        }
    }

    lines.join("\n")
}

fn format_timestamp(ts: Option<f64>) -> String {
    match ts {
        Some(ms) if ms > 0.0 => {
            let secs = (ms / 1000.0) as i64;
            let nanos = ((ms % 1000.0) * 1_000_000.0) as u32;
            match chrono::DateTime::from_timestamp(secs, nanos) {
                Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
                None => String::new(),
            }
        }
        _ => String::new(),
    }
}

pub async fn run(args: TasksArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        TasksCommands::List {
            status,
            task_type,
            limit,
            node,
        } => run_list(status, task_type, limit, node).await,
        TasksCommands::Inspect { task_id, node } => run_inspect(task_id, node).await,
    }
}

async fn run_list(
    status: Option<String>,
    task_type: Option<String>,
    limit: u32,
    node_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".taskcast");
    let mgr = NodeConfigManager::new(config_dir);

    let node = if let Some(ref name) = node_name {
        match mgr.get(name) {
            Some(n) => n,
            None => {
                return Err(format!("Node \"{name}\" not found").into());
            }
        }
    } else {
        mgr.get_current()
    };

    let client = TaskcastClient::from_node(&node).await?;

    let mut params = Vec::new();
    if let Some(ref s) = status {
        params.push(format!("status={}", s));
    }
    if let Some(ref t) = task_type {
        params.push(format!("type={}", t));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    let path = format!("/tasks{}", qs);
    let res = client.get(&path).await?;

    if !res.status().is_success() {
        let status_code = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("HTTP {} — {}", status_code.as_u16(), body).into());
    }

    let body: ListTasksResponse = res.json().await?;
    let tasks: Vec<&TaskListItem> = body.tasks.iter().take(limit as usize).collect();
    let owned: Vec<TaskListItem> = tasks
        .into_iter()
        .map(|t| TaskListItem {
            id: t.id.clone(),
            task_type: t.task_type.clone(),
            status: t.status.clone(),
            created_at: t.created_at,
        })
        .collect();
    println!("{}", format_task_list(&owned));

    Ok(())
}

async fn run_inspect(
    task_id: String,
    node_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".taskcast");
    let mgr = NodeConfigManager::new(config_dir);

    let node = if let Some(ref name) = node_name {
        match mgr.get(name) {
            Some(n) => n,
            None => {
                return Err(format!("Node \"{name}\" not found").into());
            }
        }
    } else {
        mgr.get_current()
    };

    let client = TaskcastClient::from_node(&node).await?;

    // Get task details
    let task_res = client.get(&format!("/tasks/{}", task_id)).await?;
    if !task_res.status().is_success() {
        let status_code = task_res.status();
        let body = task_res.text().await.unwrap_or_default();
        return Err(format!("HTTP {} — {}", status_code.as_u16(), body).into());
    }
    let task: TaskDetail = task_res.json().await?;

    // Get event history
    let events_res = client
        .get(&format!("/tasks/{}/events/history", task_id))
        .await?;
    let events: Vec<EventItem> = if events_res.status().is_success() {
        events_res.json().await?
    } else {
        vec![]
    };

    println!("{}", format_task_inspect(&task, &events));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_list_with_tasks() {
        let tasks = vec![
            TaskListItem {
                id: "01JXXXXXXXXXXXXXXXXXX".to_string(),
                task_type: Some("llm.chat".to_string()),
                status: "running".to_string(),
                created_at: Some(1741355401000.0),
            },
            TaskListItem {
                id: "01JYYYYYYYYYYYYYYYYYY".to_string(),
                task_type: Some("agent.step".to_string()),
                status: "completed".to_string(),
                created_at: Some(1741355335000.0),
            },
        ];
        let output = format_task_list(&tasks);

        assert!(output.contains("ID"));
        assert!(output.contains("TYPE"));
        assert!(output.contains("STATUS"));
        assert!(output.contains("CREATED"));
        assert!(output.contains("01JXXXXXXXXXXXXXXXXXX"));
        assert!(output.contains("llm.chat"));
        assert!(output.contains("running"));
        assert!(output.contains("01JYYYYYYYYYYYYYYYYYY"));
        assert!(output.contains("agent.step"));
        assert!(output.contains("completed"));
    }

    #[test]
    fn format_list_empty() {
        let output = format_task_list(&[]);
        assert_eq!(output, "No tasks found.");
    }

    #[test]
    fn format_list_missing_type() {
        let tasks = vec![TaskListItem {
            id: "01JABCDEF".to_string(),
            task_type: None,
            status: "pending".to_string(),
            created_at: Some(1741355401000.0),
        }];
        let output = format_task_list(&tasks);
        assert!(output.contains("01JABCDEF"));
        assert!(output.contains("pending"));
    }

    #[test]
    fn format_list_header_is_first_line() {
        let tasks = vec![TaskListItem {
            id: "01JXXXXXXXXXXXXXXXXXX".to_string(),
            task_type: Some("llm.chat".to_string()),
            status: "running".to_string(),
            created_at: Some(1741355401000.0),
        }];
        let output = format_task_list(&tasks);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].contains("ID"));
        assert!(lines[0].contains("TYPE"));
        assert_eq!(lines.len(), 2); // header + 1 row
    }

    #[test]
    fn format_inspect_with_events() {
        let task = TaskDetail {
            id: "01JXXXXXXXXXXXXXXXXXX".to_string(),
            task_type: Some("llm.chat".to_string()),
            status: "running".to_string(),
            params: Some(serde_json::json!({"prompt": "Hello"})),
            created_at: Some(1741355401000.0),
        };
        let events = vec![
            EventItem {
                event_type: Some("llm.delta".to_string()),
                level: Some("info".to_string()),
                series_id: Some("response".to_string()),
                timestamp: Some(1741355402000.0),
            },
            EventItem {
                event_type: Some("llm.delta".to_string()),
                level: Some("info".to_string()),
                series_id: Some("response".to_string()),
                timestamp: Some(1741355402500.0),
            },
        ];
        let output = format_task_inspect(&task, &events);

        assert!(output.contains("Task: 01JXXXXXXXXXXXXXXXXXX"));
        assert!(output.contains("Type:    llm.chat"));
        assert!(output.contains("Status:  running"));
        assert!(output.contains("Params:"));
        assert!(output.contains("prompt"));
        assert!(output.contains("Recent Events (last 2):"));
        assert!(output.contains("#0"));
        assert!(output.contains("#1"));
        assert!(output.contains("llm.delta"));
        assert!(output.contains("series:response"));
    }

    #[test]
    fn format_inspect_no_events() {
        let task = TaskDetail {
            id: "01JXXXXXXXXXXXXXXXXXX".to_string(),
            task_type: Some("llm.chat".to_string()),
            status: "pending".to_string(),
            params: None,
            created_at: Some(1741355401000.0),
        };
        let output = format_task_inspect(&task, &[]);

        assert!(output.contains("Task: 01JXXXXXXXXXXXXXXXXXX"));
        assert!(output.contains("Type:    llm.chat"));
        assert!(output.contains("Status:  pending"));
        assert!(output.contains("No events."));
        assert!(!output.contains("Recent Events"));
    }

    #[test]
    fn format_inspect_shows_only_last_5_events() {
        let task = TaskDetail {
            id: "01JABCDEF".to_string(),
            task_type: Some("batch".to_string()),
            status: "running".to_string(),
            params: Some(serde_json::json!({})),
            created_at: Some(1741355401000.0),
        };
        let events: Vec<EventItem> = (0..8)
            .map(|i| EventItem {
                event_type: Some(format!("step.{}", i)),
                level: Some("info".to_string()),
                series_id: None,
                timestamp: Some(1741355402000.0 + i as f64 * 1000.0),
            })
            .collect();
        let output = format_task_inspect(&task, &events);

        assert!(output.contains("Recent Events (last 5):"));
        assert!(output.contains("#0"));
        assert!(output.contains("#4"));
        assert!(!output.contains("#5"));
        // The last 5 events are step.3 through step.7
        assert!(output.contains("step.3"));
        assert!(output.contains("step.7"));
        assert!(!output.contains("step.2"));
    }

    #[test]
    fn format_inspect_without_series_id() {
        let task = TaskDetail {
            id: "01JABCDEF".to_string(),
            task_type: Some("llm.chat".to_string()),
            status: "running".to_string(),
            params: Some(serde_json::json!({})),
            created_at: Some(1741355401000.0),
        };
        let events = vec![EventItem {
            event_type: Some("log".to_string()),
            level: Some("info".to_string()),
            series_id: None,
            timestamp: Some(1741355402000.0),
        }];
        let output = format_task_inspect(&task, &events);

        assert!(output.contains("#0"));
        assert!(output.contains("log"));
        assert!(!output.contains("series:"));
    }

    #[test]
    fn format_inspect_with_null_params() {
        let task = TaskDetail {
            id: "01JABCDEF".to_string(),
            task_type: Some("simple".to_string()),
            status: "completed".to_string(),
            params: Some(serde_json::Value::Null),
            created_at: Some(1741355401000.0),
        };
        let output = format_task_inspect(&task, &[]);

        assert!(output.contains("Params:  "));
        assert!(output.contains("Status:  completed"));
    }

    #[test]
    fn format_timestamp_renders_correctly() {
        let ts = format_timestamp(Some(1741355401000.0));
        // Should be a date-time string in UTC
        assert!(ts.contains("2025"), "expected year 2025, got: {ts}");
        assert!(ts.contains(":"), "expected time separator, got: {ts}");
    }

    #[test]
    fn format_timestamp_none_returns_empty() {
        assert_eq!(format_timestamp(None), "");
    }

    #[test]
    fn format_timestamp_zero_returns_empty() {
        assert_eq!(format_timestamp(Some(0.0)), "");
    }
}
