use std::fs;
use std::process::Command;

use super::manager::{ServiceInstallOptions, ServiceManager, ServiceStatus};
use super::paths::ServicePaths;

pub struct SystemdServiceManager;

fn quote_if_needed(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

pub fn generate_unit_file(opts: &ServiceInstallOptions) -> String {
    let mut exec_parts = vec![
        quote_if_needed(&opts.exec_path),
        "start".to_string(),
        "--port".to_string(),
        opts.port.to_string(),
    ];

    if let Some(ref config) = opts.config {
        exec_parts.push("--config".to_string());
        exec_parts.push(quote_if_needed(config));
    }
    if let Some(ref storage) = opts.storage {
        exec_parts.push("--storage".to_string());
        exec_parts.push(quote_if_needed(storage));
    }
    if let Some(ref db_path) = opts.db_path {
        exec_parts.push("--db-path".to_string());
        exec_parts.push(quote_if_needed(db_path));
    }

    let exec_start = exec_parts.join(" ");

    format!(
        "[Unit]\n\
         Description=Taskcast \u{2014} unified task tracking and streaming service\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec_start}\n\
         Restart=no\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

pub fn parse_systemctl_status(output: &str) -> ServiceStatus {
    let mut active_state = "";
    let mut main_pid: u32 = 0;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("ActiveState=") {
            active_state = rest.trim();
        } else if let Some(rest) = line.strip_prefix("MainPID=") {
            main_pid = rest.trim().parse().unwrap_or(0);
        }
    }

    if active_state == "active" && main_pid > 0 {
        ServiceStatus::Running { pid: main_pid }
    } else {
        ServiceStatus::Stopped
    }
}

fn systemctl(args: &[&str]) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut cmd_args = vec!["--user"];
    cmd_args.extend_from_slice(args);
    let output = Command::new("systemctl").args(&cmd_args).output()?;
    Ok(output)
}

impl ServiceManager for SystemdServiceManager {
    fn install(&self, opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if paths.is_installed() {
            return Err("Taskcast service is already installed. Run `taskcast service uninstall` first.".into());
        }

        if let Some(parent) = paths.service_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let unit = generate_unit_file(opts);
        fs::write(&paths.service_file, unit)?;

        let output = systemctl(&["daemon-reload"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("systemctl daemon-reload failed: {stderr}").into());
        }

        let output = systemctl(&["enable", "taskcast"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("systemctl enable failed: {stderr}").into());
        }

        Ok(())
    }

    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Err("Taskcast service is not installed.".into());
        }

        let _ = systemctl(&["disable", "taskcast"]);
        let _ = self.stop();

        systemctl(&["daemon-reload"])?;
        fs::remove_file(&paths.service_file)?;
        Ok(())
    }

    fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Err("Taskcast service is not installed. Run `taskcast service install` first.".into());
        }

        let output = systemctl(&["start", "taskcast"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("systemctl start failed: {stderr}").into());
        }
        Ok(())
    }

    fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let output = systemctl(&["stop", "taskcast"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("not loaded") {
                return Err(format!("systemctl stop failed: {stderr}").into());
            }
        }
        Ok(())
    }

    fn restart(&self) -> Result<(), Box<dyn std::error::Error>> {
        let output = systemctl(&["restart", "taskcast"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("systemctl restart failed: {stderr}").into());
        }
        Ok(())
    }

    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Ok(ServiceStatus::NotInstalled);
        }

        let output = systemctl(&["show", "taskcast", "--property=ActiveState,MainPID"])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_systemctl_status(&stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_file_minimal_options() {
        let opts = ServiceInstallOptions {
            port: 3721, config: None, storage: None, db_path: None,
            exec_path: "/usr/local/bin/taskcast".into(),
        };
        let unit = generate_unit_file(&opts);
        assert!(unit.contains("Description=Taskcast"));
        assert!(unit.contains("After=network.target"));
        assert!(unit.contains("Type=simple"));
        assert!(unit.contains("ExecStart=/usr/local/bin/taskcast start --port 3721"));
        assert!(unit.contains("Restart=no"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("--config"));
        assert!(!unit.contains("--storage"));
        assert!(!unit.contains("--db-path"));
    }

    #[test]
    fn unit_file_all_options() {
        let opts = ServiceInstallOptions {
            port: 8080,
            config: Some("/etc/taskcast.yaml".into()),
            storage: Some("sqlite".into()),
            db_path: Some("/data/tasks.db".into()),
            exec_path: "/opt/taskcast".into(),
        };
        let unit = generate_unit_file(&opts);
        assert!(unit.contains("ExecStart=/opt/taskcast start --port 8080 --config /etc/taskcast.yaml --storage sqlite --db-path /data/tasks.db"));
    }

    #[test]
    fn unit_file_quotes_paths_with_spaces() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: Some("/path with spaces/config.yaml".into()),
            storage: None, db_path: None,
            exec_path: "/usr/local/bin/taskcast".into(),
        };
        let unit = generate_unit_file(&opts);
        assert!(unit.contains("--config \"/path with spaces/config.yaml\""));
    }

    #[test]
    fn parse_status_running() {
        let output = "ActiveState=active\nMainPID=42\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Running { pid: 42 });
    }

    #[test]
    fn parse_status_stopped() {
        let output = "ActiveState=inactive\nMainPID=0\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Stopped);
    }

    #[test]
    fn parse_status_active_but_no_pid() {
        let output = "ActiveState=active\nMainPID=0\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Stopped);
    }

    #[test]
    fn parse_status_empty_output() {
        assert_eq!(parse_systemctl_status(""), ServiceStatus::Stopped);
    }

    #[test]
    fn parse_status_failed_state() {
        let output = "ActiveState=failed\nMainPID=0\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Stopped);
    }

    #[test]
    fn quote_if_needed_no_spaces() {
        assert_eq!(quote_if_needed("/usr/bin/taskcast"), "/usr/bin/taskcast");
    }

    #[test]
    fn quote_if_needed_with_spaces() {
        assert_eq!(quote_if_needed("/path with spaces"), "\"/path with spaces\"");
    }
}
