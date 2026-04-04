pub mod manager;
pub mod paths;
pub mod health;

#[cfg(target_os = "macos")]
pub mod launchd;

#[cfg(target_os = "linux")]
pub mod systemd;

use clap::{Args, Subcommand};

use self::manager::{create_service_manager, ServiceInstallOptions, ServiceStatus};
use self::paths::{ServicePaths, delete_state, ensure_config, get_port, write_state};
use self::health::{fetch_health_detail, format_uptime, poll_health};

const HEALTH_TIMEOUT_MS: u64 = 5000;
const HEALTH_INTERVAL_MS: u64 = 500;

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommands,
}

#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Register and enable the Taskcast service for auto-start
    Install {
        /// Path to config file
        #[arg(short = 'c', long)]
        config: Option<String>,

        /// Port to listen on
        #[arg(short = 'p', long, default_value_t = 3721)]
        port: u16,

        /// Storage backend: memory | redis | sqlite
        #[arg(short = 's', long)]
        storage: Option<String>,

        /// SQLite database file path
        #[arg(long)]
        db_path: Option<String>,
    },
    /// Remove the Taskcast service registration
    Uninstall,
    /// Start the Taskcast service
    Start,
    /// Stop the Taskcast service
    Stop,
    /// Restart the Taskcast service
    Restart,
    /// Reload the Taskcast service (alias for restart)
    Reload,
    /// Show Taskcast service status
    Status,
}

pub async fn run(args: ServiceArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        ServiceCommands::Install { config, port, storage, db_path } => {
            run_install(config, port, storage, db_path)?;
        }
        ServiceCommands::Uninstall => {
            run_uninstall()?;
        }
        ServiceCommands::Start => {
            run_start().await?;
        }
        ServiceCommands::Stop => {
            run_stop()?;
        }
        ServiceCommands::Restart | ServiceCommands::Reload => {
            run_restart().await?;
        }
        ServiceCommands::Status => {
            run_status().await?;
        }
    }
    Ok(())
}

fn run_install(
    config: Option<String>,
    port: u16,
    storage: Option<String>,
    db_path: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if port == 0 {
        eprintln!("[taskcast] Invalid port: {port}");
        std::process::exit(1);
    }

    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    let config_path = ensure_config(&paths, config.as_deref())?;

    // Default to sqlite if no explicit config or storage specified
    let storage = if config.is_none() && storage.is_none() {
        Some("sqlite".to_string())
    } else {
        storage
    };

    let exec_path = std::env::current_exe()?
        .to_string_lossy()
        .into_owned();

    let opts = ServiceInstallOptions {
        port,
        config: Some(config_path),
        storage,
        db_path,
        exec_path,
    };

    mgr.install(&opts)?;
    write_state(&paths, port)?;

    eprintln!("[taskcast] Service installed successfully.");
    eprintln!("[taskcast] Run 'taskcast service start' to start the service.");
    Ok(())
}

fn run_uninstall() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    mgr.uninstall()?;
    delete_state(&paths);

    eprintln!("[taskcast] Service uninstalled.");
    Ok(())
}

pub async fn run_start() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    let status = mgr.status()?;
    if let ServiceStatus::Running { .. } = status {
        eprintln!("[taskcast] Service is already running.");
        return Ok(());
    }

    mgr.start()?;

    let port = get_port(&paths);
    if poll_health(port, HEALTH_TIMEOUT_MS, HEALTH_INTERVAL_MS).await {
        eprintln!("[taskcast] Service started on http://localhost:{port}");
    } else {
        eprintln!("[taskcast] Service may have failed to start. Check logs:");
        print_log_hint(&paths);
    }

    Ok(())
}

pub fn run_stop() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;

    let status = mgr.status()?;
    match status {
        ServiceStatus::NotInstalled => {
            eprintln!("[taskcast] Service is not installed.");
            return Ok(());
        }
        ServiceStatus::Stopped => {
            eprintln!("[taskcast] Service is not running.");
            return Ok(());
        }
        _ => {}
    }

    mgr.stop()?;
    eprintln!("[taskcast] Service stopped.");
    Ok(())
}

async fn run_restart() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    mgr.restart()?;

    let port = get_port(&paths);
    if poll_health(port, HEALTH_TIMEOUT_MS, HEALTH_INTERVAL_MS).await {
        eprintln!("[taskcast] Service started on http://localhost:{port}");
    } else {
        eprintln!("[taskcast] Service may have failed to start. Check logs:");
        print_log_hint(&paths);
    }

    Ok(())
}

pub async fn run_status() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    let status = mgr.status()?;
    eprintln!("Service:   {status}");

    if let ServiceStatus::Running { .. } = &status {
        let port = get_port(&paths);
        if let Some((uptime, storage)) = fetch_health_detail(port).await {
            if let Some(secs) = uptime {
                eprintln!("Uptime:    {}", format_uptime(secs));
            }
            if let Some(provider) = storage {
                eprintln!("Storage:   {provider}");
            }
        }
    }

    Ok(())
}

fn print_log_hint(paths: &ServicePaths) {
    if let Some(ref stdout_log) = paths.stdout_log {
        eprintln!("  {}", stdout_log.display());
    }
    if let Some(ref stderr_log) = paths.stderr_log {
        eprintln!("  {}", stderr_log.display());
    }
    if paths.stdout_log.is_none() {
        eprintln!("  journalctl --user -u taskcast");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_commands_install_defaults() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: ServiceCommands,
        }

        let cli = TestCli::parse_from(["test", "install"]);
        match cli.cmd {
            ServiceCommands::Install { port, config, storage, db_path } => {
                assert_eq!(port, 3721);
                assert!(config.is_none());
                assert!(storage.is_none());
                assert!(db_path.is_none());
            }
            _ => panic!("expected Install"),
        }
    }

    #[test]
    fn service_commands_install_with_all_flags() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: ServiceCommands,
        }

        let cli = TestCli::parse_from([
            "test", "install",
            "-c", "/etc/tc.yaml",
            "-p", "8080",
            "-s", "sqlite",
            "--db-path", "/data/tc.db",
        ]);
        match cli.cmd {
            ServiceCommands::Install { config, port, storage, db_path } => {
                assert_eq!(config, Some("/etc/tc.yaml".to_string()));
                assert_eq!(port, 8080);
                assert_eq!(storage, Some("sqlite".to_string()));
                assert_eq!(db_path, Some("/data/tc.db".to_string()));
            }
            _ => panic!("expected Install"),
        }
    }

    #[test]
    fn service_commands_all_variants_parse() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: ServiceCommands,
        }

        for cmd in ["uninstall", "start", "stop", "restart", "reload", "status"] {
            let cli = TestCli::parse_from(["test", cmd]);
            match (cmd, cli.cmd) {
                ("uninstall", ServiceCommands::Uninstall) => {},
                ("start", ServiceCommands::Start) => {},
                ("stop", ServiceCommands::Stop) => {},
                ("restart", ServiceCommands::Restart) => {},
                ("reload", ServiceCommands::Reload) => {},
                ("status", ServiceCommands::Status) => {},
                _ => panic!("unexpected parse for {cmd}"),
            }
        }
    }
}
