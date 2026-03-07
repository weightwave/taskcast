use clap::Args;
use std::collections::HashMap;

use crate::node_config::NodeEntry;

#[derive(Args)]
pub struct DoctorArgs {
    /// Node name to check (default: current node)
    #[arg(long)]
    pub node: Option<String>,
}

pub struct ServerStatus {
    pub ok: bool,
    pub url: String,
    pub uptime: Option<u64>,
    pub error: Option<String>,
}

pub struct AuthStatus {
    pub status: String, // "ok" or "warn"
    pub mode: Option<String>,
    pub message: Option<String>,
}

pub struct AdapterStatus {
    pub name: String,
    pub provider: String,
    pub status: String,
}

pub struct DoctorResult {
    pub server: ServerStatus,
    pub auth: AuthStatus,
    pub adapters: Vec<AdapterStatus>,
}

#[derive(serde::Deserialize)]
struct HealthDetailAuth {
    mode: String,
}

#[derive(serde::Deserialize)]
struct HealthDetailAdapter {
    provider: String,
    status: String,
}

#[derive(serde::Deserialize)]
struct HealthDetailResponse {
    #[allow(dead_code)]
    ok: bool,
    uptime: u64,
    auth: HealthDetailAuth,
    adapters: HashMap<String, HealthDetailAdapter>,
}

pub async fn run_doctor(node: &NodeEntry) -> DoctorResult {
    let url = node.url.trim_end_matches('/');
    let endpoint = format!("{url}/health/detail");

    let client = reqwest::Client::new();
    let res = match client.get(&endpoint).send().await {
        Ok(res) => res,
        Err(e) => {
            return DoctorResult {
                server: ServerStatus {
                    ok: false,
                    url: url.to_string(),
                    uptime: None,
                    error: Some(e.to_string()),
                },
                auth: AuthStatus {
                    status: "warn".to_string(),
                    mode: None,
                    message: None,
                },
                adapters: vec![],
            };
        }
    };

    if !res.status().is_success() {
        return DoctorResult {
            server: ServerStatus {
                ok: false,
                url: url.to_string(),
                uptime: None,
                error: Some(format!("HTTP {}", res.status().as_u16())),
            },
            auth: AuthStatus {
                status: "warn".to_string(),
                mode: None,
                message: None,
            },
            adapters: vec![],
        };
    }

    let body: HealthDetailResponse = match res.json().await {
        Ok(b) => b,
        Err(e) => {
            return DoctorResult {
                server: ServerStatus {
                    ok: false,
                    url: url.to_string(),
                    uptime: None,
                    error: Some(format!("failed to parse response: {e}")),
                },
                auth: AuthStatus {
                    status: "warn".to_string(),
                    mode: None,
                    message: None,
                },
                adapters: vec![],
            };
        }
    };

    let auth_warn = node.token.is_none() && body.auth.mode != "none";
    let auth_status = if auth_warn { "warn" } else { "ok" };
    let auth_message = if auth_warn {
        Some("no token configured for this node".to_string())
    } else {
        None
    };

    // Collect adapters in canonical order
    let adapter_names = ["broadcast", "shortTermStore", "longTermStore"];
    let mut adapters = Vec::new();
    for name in &adapter_names {
        if let Some(adapter) = body.adapters.get(*name) {
            adapters.push(AdapterStatus {
                name: name.to_string(),
                provider: adapter.provider.clone(),
                status: adapter.status.clone(),
            });
        }
    }

    DoctorResult {
        server: ServerStatus {
            ok: true,
            url: url.to_string(),
            uptime: Some(body.uptime),
            error: None,
        },
        auth: AuthStatus {
            status: auth_status.to_string(),
            mode: Some(body.auth.mode),
            message: auth_message,
        },
        adapters,
    }
}

pub fn format_doctor_result(result: &DoctorResult) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Server line
    if result.server.ok {
        let uptime_str = match result.server.uptime {
            Some(s) => format!(" (uptime: {s}s)"),
            None => String::new(),
        };
        lines.push(format!(
            "Server:    OK  taskcast at {}{}",
            result.server.url, uptime_str
        ));
    } else {
        let error = result.server.error.as_deref().unwrap_or("unknown error");
        lines.push(format!(
            "Server:    FAIL  cannot reach {}: {}",
            result.server.url, error
        ));
    }

    // Auth line
    if result.auth.status == "ok" {
        let mode = result.auth.mode.as_deref().unwrap_or("unknown");
        lines.push(format!("Auth:      OK  {mode}"));
    } else {
        let msg = result
            .auth
            .message
            .as_deref()
            .or(result.auth.mode.as_deref())
            .unwrap_or("unknown");
        lines.push(format!("Auth:      WARN  {msg}"));
    }

    // Adapter lines
    let label_map: &[(&str, &str)] = &[
        ("broadcast", "Broadcast"),
        ("shortTermStore", "ShortTerm"),
        ("longTermStore", "LongTerm"),
    ];

    for (key, label) in label_map {
        let padded_label = format!("{label}:");
        let padded_label = format!("{padded_label:<11}");

        if let Some(adapter) = result.adapters.iter().find(|a| a.name == *key) {
            let status_tag = if adapter.status == "ok" { "OK" } else { "FAIL" };
            lines.push(format!("{padded_label}{status_tag}  {}", adapter.provider));
        } else if *key == "longTermStore" {
            lines.push(format!("{padded_label}SKIP  not configured"));
        }
    }

    lines.join("\n")
}

pub async fn run(args: DoctorArgs) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".taskcast");
    let mgr = crate::node_config::NodeConfigManager::new(config_dir);

    let node = if let Some(ref name) = args.node {
        match mgr.get(name) {
            Some(n) => n,
            None => {
                eprintln!("Node \"{name}\" not found");
                std::process::exit(1);
            }
        }
    } else {
        mgr.get_current()
    };

    let result = run_doctor(&node).await;
    println!("{}", format_doctor_result(&result));

    if !result.server.ok {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_all_ok() {
        let result = DoctorResult {
            server: ServerStatus {
                ok: true,
                url: "http://localhost:3721".to_string(),
                uptime: Some(120),
                error: None,
            },
            auth: AuthStatus {
                status: "ok".to_string(),
                mode: Some("none".to_string()),
                message: None,
            },
            adapters: vec![
                AdapterStatus {
                    name: "broadcast".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
                AdapterStatus {
                    name: "shortTermStore".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
            ],
        };
        let output = format_doctor_result(&result);
        assert!(output.contains("Server:    OK  taskcast at http://localhost:3721 (uptime: 120s)"));
        assert!(output.contains("Auth:      OK  none"));
        assert!(output.contains("Broadcast: OK  memory"));
        assert!(output.contains("ShortTerm: OK  memory"));
        assert!(output.contains("LongTerm:  SKIP  not configured"));
    }

    #[test]
    fn format_server_fail() {
        let result = DoctorResult {
            server: ServerStatus {
                ok: false,
                url: "http://localhost:3721".to_string(),
                uptime: None,
                error: Some("ECONNREFUSED".to_string()),
            },
            auth: AuthStatus {
                status: "warn".to_string(),
                mode: None,
                message: None,
            },
            adapters: vec![],
        };
        let output = format_doctor_result(&result);
        assert!(output.contains("Server:    FAIL  cannot reach http://localhost:3721: ECONNREFUSED"));
        assert!(output.contains("Auth:      WARN"));
    }

    #[test]
    fn format_auth_warn_with_message() {
        let result = DoctorResult {
            server: ServerStatus {
                ok: true,
                url: "http://localhost:3721".to_string(),
                uptime: Some(60),
                error: None,
            },
            auth: AuthStatus {
                status: "warn".to_string(),
                mode: Some("jwt".to_string()),
                message: Some("no token configured for this node".to_string()),
            },
            adapters: vec![
                AdapterStatus {
                    name: "broadcast".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
                AdapterStatus {
                    name: "shortTermStore".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
            ],
        };
        let output = format_doctor_result(&result);
        assert!(output.contains("Auth:      WARN  no token configured for this node"));
    }

    #[test]
    fn format_with_long_term_store() {
        let result = DoctorResult {
            server: ServerStatus {
                ok: true,
                url: "http://localhost:3721".to_string(),
                uptime: Some(300),
                error: None,
            },
            auth: AuthStatus {
                status: "ok".to_string(),
                mode: Some("none".to_string()),
                message: None,
            },
            adapters: vec![
                AdapterStatus {
                    name: "broadcast".to_string(),
                    provider: "redis".to_string(),
                    status: "ok".to_string(),
                },
                AdapterStatus {
                    name: "shortTermStore".to_string(),
                    provider: "redis".to_string(),
                    status: "ok".to_string(),
                },
                AdapterStatus {
                    name: "longTermStore".to_string(),
                    provider: "postgres".to_string(),
                    status: "ok".to_string(),
                },
            ],
        };
        let output = format_doctor_result(&result);
        assert!(output.contains("Broadcast: OK  redis"));
        assert!(output.contains("ShortTerm: OK  redis"));
        assert!(output.contains("LongTerm:  OK  postgres"));
        assert!(!output.contains("SKIP"));
    }

    #[test]
    fn format_skip_long_term_when_not_configured() {
        let result = DoctorResult {
            server: ServerStatus {
                ok: true,
                url: "http://localhost:3721".to_string(),
                uptime: Some(10),
                error: None,
            },
            auth: AuthStatus {
                status: "ok".to_string(),
                mode: Some("none".to_string()),
                message: None,
            },
            adapters: vec![
                AdapterStatus {
                    name: "broadcast".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
                AdapterStatus {
                    name: "shortTermStore".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
            ],
        };
        let output = format_doctor_result(&result);
        assert!(output.contains("LongTerm:  SKIP  not configured"));
    }

    #[test]
    fn format_omits_uptime_when_not_provided() {
        let result = DoctorResult {
            server: ServerStatus {
                ok: true,
                url: "http://localhost:3721".to_string(),
                uptime: None,
                error: None,
            },
            auth: AuthStatus {
                status: "ok".to_string(),
                mode: Some("none".to_string()),
                message: None,
            },
            adapters: vec![
                AdapterStatus {
                    name: "broadcast".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
                AdapterStatus {
                    name: "shortTermStore".to_string(),
                    provider: "memory".to_string(),
                    status: "ok".to_string(),
                },
            ],
        };
        let output = format_doctor_result(&result);
        assert!(output.contains("Server:    OK  taskcast at http://localhost:3721"));
        assert!(!output.contains("uptime"));
    }
}
