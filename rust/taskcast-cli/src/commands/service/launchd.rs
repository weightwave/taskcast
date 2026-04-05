use std::fs;
use std::process::Command;

use super::manager::{ServiceInstallOptions, ServiceManager, ServiceStatus};
use super::paths::ServicePaths;

const LAUNCHD_LABEL: &str = "com.taskcast.daemon";

pub struct LaunchdServiceManager;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn generate_plist(opts: &ServiceInstallOptions, stdout_log: &str, stderr_log: &str) -> String {
    let mut args = vec![
        format!("        <string>{}</string>", xml_escape(&opts.exec_path)),
        "        <string>start</string>".to_string(),
        "        <string>--port</string>".to_string(),
        format!("        <string>{}</string>", opts.port),
    ];

    if let Some(ref config) = opts.config {
        args.push("        <string>--config</string>".to_string());
        args.push(format!("        <string>{}</string>", xml_escape(config)));
    }
    if let Some(ref storage) = opts.storage {
        args.push("        <string>--storage</string>".to_string());
        args.push(format!("        <string>{}</string>", xml_escape(storage)));
    }
    if let Some(ref db_path) = opts.db_path {
        args.push("        <string>--db-path</string>".to_string());
        args.push(format!("        <string>{}</string>", xml_escape(db_path)));
    }

    let args_xml = args.join("\n");
    let stdout_log = xml_escape(stdout_log);
    let stderr_log = xml_escape(stderr_log);

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>{stdout_log}</string>
    <key>StandardErrorPath</key>
    <string>{stderr_log}</string>
</dict>
</plist>
"#
    )
}

pub fn parse_launchctl_pid(output: &str) -> Option<u32> {
    for line in output.lines() {
        let trimmed = line.trim().trim_end_matches(';');
        if let Some(rest) = trimmed.strip_prefix("\"PID\"") {
            let rest = rest.trim().strip_prefix('=')?.trim();
            return rest.parse().ok();
        }
    }
    None
}

fn get_uid() -> u32 {
    unsafe { libc::getuid() }
}

impl ServiceManager for LaunchdServiceManager {
    fn install(&self, opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if paths.is_installed() {
            return Err("Taskcast service is already installed. Run `taskcast service uninstall` first.".into());
        }

        if let Some(ref log_dir) = paths.log_dir {
            fs::create_dir_all(log_dir)?;
        }
        if let Some(parent) = paths.service_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let stdout_log = paths
            .stdout_log
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stderr_log = paths
            .stderr_log
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let plist = generate_plist(opts, &stdout_log, &stderr_log);
        fs::write(&paths.service_file, plist)?;

        Ok(())
    }

    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;
        if !paths.is_installed() {
            return Err("Taskcast service is not installed.".into());
        }
        let _ = self.stop();
        fs::remove_file(&paths.service_file)?;
        Ok(())
    }

    fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;
        if !paths.is_installed() {
            return Err(
                "Taskcast service is not installed. Run `taskcast service install` first.".into(),
            );
        }

        let uid = get_uid();
        let plist_path = paths.service_file.to_string_lossy();

        let output = Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}"), &plist_path])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("already loaded") {
                return Err(format!("launchctl bootstrap failed: {stderr}").into());
            }
        }
        Ok(())
    }

    fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let uid = get_uid();
        let output = Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("not found") {
                return Err(format!("launchctl bootout failed: {stderr}").into());
            }
        }
        Ok(())
    }

    fn restart(&self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.stop();
        self.start()
    }

    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;
        if !paths.is_installed() {
            return Ok(ServiceStatus::NotInstalled);
        }

        let output = Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()?;

        if !output.status.success() {
            return Ok(ServiceStatus::Stopped);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        match parse_launchctl_pid(&stdout) {
            Some(pid) if pid > 0 => Ok(ServiceStatus::Running { pid }),
            _ => Ok(ServiceStatus::Stopped),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_minimal_options() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/local/bin/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/tmp/taskcast.log", "/tmp/taskcast.err.log");
        assert!(plist.contains("<string>com.taskcast.daemon</string>"));
        assert!(plist.contains("<string>/usr/local/bin/taskcast</string>"));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("<string>--port</string>"));
        assert!(plist.contains("<string>3721</string>"));
        assert!(plist.contains("<true/>"));
        assert!(plist.contains("<false/>"));
        assert!(!plist.contains("--config"));
        assert!(!plist.contains("--storage"));
        assert!(!plist.contains("--db-path"));
    }

    #[test]
    fn plist_all_options() {
        let opts = ServiceInstallOptions {
            port: 8080,
            config: Some("/etc/taskcast.yaml".into()),
            storage: Some("sqlite".into()),
            db_path: Some("/data/tasks.db".into()),
            exec_path: "/opt/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/log/out", "/log/err");
        assert!(plist.contains("<string>--config</string>"));
        assert!(plist.contains("<string>/etc/taskcast.yaml</string>"));
        assert!(plist.contains("<string>--storage</string>"));
        assert!(plist.contains("<string>sqlite</string>"));
        assert!(plist.contains("<string>--db-path</string>"));
        assert!(plist.contains("<string>/data/tasks.db</string>"));
        assert!(plist.contains("<string>8080</string>"));
    }

    #[test]
    fn plist_is_valid_xml_structure() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/bin/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/tmp/out", "/tmp/err");
        assert!(plist.starts_with("<?xml version=\"1.0\""));
        assert!(plist.contains("</plist>"));
        assert!(plist.contains("<!DOCTYPE plist"));
    }

    #[test]
    fn parse_pid_from_launchctl_output() {
        let output = "{\n\t\"PID\" = 12345;\n\t\"Label\" = \"com.taskcast.daemon\";\n};";
        assert_eq!(parse_launchctl_pid(output), Some(12345));
    }

    #[test]
    fn parse_pid_returns_none_when_not_running() {
        let output =
            "{\n\t\"Label\" = \"com.taskcast.daemon\";\n\t\"LastExitStatus\" = 256;\n};";
        assert_eq!(parse_launchctl_pid(output), None);
    }

    #[test]
    fn parse_pid_returns_none_for_empty() {
        assert_eq!(parse_launchctl_pid(""), None);
    }

    #[test]
    fn parse_pid_handles_zero() {
        let output = "{\n\t\"PID\" = 0;\n};";
        assert_eq!(parse_launchctl_pid(output), Some(0));
    }

    #[test]
    fn plist_escapes_xml_special_chars() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: Some("/path/with&special<chars>".into()),
            storage: None,
            db_path: None,
            exec_path: "/usr/bin/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/tmp/out", "/tmp/err");
        assert!(plist.contains("&amp;special&lt;chars&gt;"));
        assert!(!plist.contains("&special<chars>"));
    }

    #[test]
    fn xml_escape_handles_all_entities() {
        assert_eq!(xml_escape("a&b<c>d\"e"), "a&amp;b&lt;c&gt;d&quot;e");
    }

    #[test]
    fn xml_escape_noop_for_normal_strings() {
        assert_eq!(xml_escape("/usr/local/bin/taskcast"), "/usr/local/bin/taskcast");
    }
}
