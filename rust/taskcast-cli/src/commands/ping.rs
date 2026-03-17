use clap::Args;

use crate::node_config::NodeConfigManager;

#[derive(Args, Debug)]
pub struct PingArgs {
    /// Named node to ping
    #[arg(long)]
    pub node: Option<String>,
}

pub struct PingResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

pub async fn ping_server(url: &str) -> PingResult {
    let start = std::time::Instant::now();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    match client.get(format!("{}/health", url)).send().await {
        Ok(res) if res.status().is_success() => PingResult {
            ok: true,
            latency_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(res) => PingResult {
            ok: false,
            latency_ms: None,
            error: Some(format!("HTTP {}", res.status().as_u16())),
        },
        Err(e) => PingResult {
            ok: false,
            latency_ms: None,
            error: Some(e.to_string()),
        },
    }
}

pub async fn run(args: PingArgs) {
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

    let result = ping_server(&node.url).await;
    if result.ok {
        println!(
            "OK — taskcast at {} ({}ms)",
            node.url,
            result.latency_ms.unwrap()
        );
    } else {
        eprintln!(
            "FAIL — cannot reach {}: {}",
            node.url,
            result.error.unwrap()
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ping_server_connection_refused() {
        // Use a port that is almost certainly not listening
        let result = ping_server("http://127.0.0.1:19999").await;
        assert!(!result.ok);
        assert!(result.latency_ms.is_none());
        assert!(result.error.is_some());
        let err = result.error.unwrap();
        // The error message should contain some connection-related text
        assert!(
            err.contains("error") || err.contains("connect") || err.contains("Connection"),
            "expected connection error, got: {err}"
        );
    }
}
