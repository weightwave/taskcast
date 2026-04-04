use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_PORT: u16 = 3721;

pub struct ServicePaths {
    pub service_file: PathBuf,
    pub log_dir: Option<PathBuf>,
    pub stdout_log: Option<PathBuf>,
    pub stderr_log: Option<PathBuf>,
    pub default_config: PathBuf,
    pub default_db: PathBuf,
    pub state_file: PathBuf,
}

impl ServicePaths {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let home = dirs::home_dir().ok_or("cannot determine home directory")?;
        let taskcast_dir = home.join(".taskcast");

        #[cfg(target_os = "macos")]
        {
            let log_dir = home.join("Library/Application Support/taskcast");
            Ok(ServicePaths {
                service_file: home.join("Library/LaunchAgents/com.taskcast.daemon.plist"),
                stdout_log: Some(log_dir.join("taskcast.log")),
                stderr_log: Some(log_dir.join("taskcast.err.log")),
                log_dir: Some(log_dir),
                default_config: taskcast_dir.join("taskcast.config.yaml"),
                default_db: taskcast_dir.join("taskcast.db"),
                state_file: taskcast_dir.join("service.state.json"),
            })
        }

        #[cfg(target_os = "linux")]
        {
            Ok(ServicePaths {
                service_file: home.join(".config/systemd/user/taskcast.service"),
                log_dir: None,
                stdout_log: None,
                stderr_log: None,
                default_config: taskcast_dir.join("taskcast.config.yaml"),
                default_db: taskcast_dir.join("taskcast.db"),
                state_file: taskcast_dir.join("service.state.json"),
            })
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Err("Service paths are not defined for this platform".into())
        }
    }

    pub fn is_installed(&self) -> bool {
        self.service_file.exists()
    }
}

pub fn ensure_config(paths: &ServicePaths, config_opt: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(config) = config_opt {
        if !Path::new(config).exists() {
            return Err(format!("Config file not found: {config}").into());
        }
        return Ok(config.to_string());
    }

    let config_path = &paths.default_config;
    if config_path.exists() {
        return Ok(config_path.to_string_lossy().into_owned());
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let db_path = paths.default_db.to_string_lossy();
    let content = format!(
        "# Taskcast service configuration\n\
         port: {DEFAULT_PORT}\n\
         \n\
         adapters:\n\
         \x20 shortTermStore:\n\
         \x20\x20\x20 provider: sqlite\n\
         \x20\x20\x20 path: {db_path}\n\
         \x20 longTermStore:\n\
         \x20\x20\x20 provider: sqlite\n\
         \x20\x20\x20 path: {db_path}\n"
    );

    fs::write(config_path, &content)?;
    eprintln!("[taskcast] Created default config at {}", config_path.display());
    Ok(config_path.to_string_lossy().into_owned())
}

pub fn write_state(paths: &ServicePaths, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = paths.state_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::json!({ "port": port });
    fs::write(&paths.state_file, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

pub fn read_state_port(paths: &ServicePaths) -> Option<u16> {
    let content = fs::read_to_string(&paths.state_file).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value.get("port")?.as_u64().map(|p| p as u16)
}

pub fn delete_state(paths: &ServicePaths) {
    let _ = fs::remove_file(&paths.state_file);
}

pub fn get_port(paths: &ServicePaths) -> u16 {
    if let Some(port) = read_state_port(paths) {
        return port;
    }
    if let Ok(content) = fs::read_to_string(&paths.default_config) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("port:") {
                if let Ok(port) = rest.trim().parse::<u16>() {
                    return port;
                }
            }
        }
    }
    DEFAULT_PORT
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn test_paths(tmp: &Path) -> ServicePaths {
        ServicePaths {
            service_file: tmp.join("service-file"),
            log_dir: Some(tmp.join("logs")),
            stdout_log: Some(tmp.join("logs/taskcast.log")),
            stderr_log: Some(tmp.join("logs/taskcast.err.log")),
            default_config: tmp.join("taskcast.config.yaml"),
            default_db: tmp.join("taskcast.db"),
            state_file: tmp.join("service.state.json"),
        }
    }

    #[test]
    fn paths_new_succeeds_on_supported_platform() {
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let paths = ServicePaths::new().unwrap();
            assert!(!paths.service_file.to_string_lossy().is_empty());
            assert!(paths.default_config.to_string_lossy().contains(".taskcast"));
        }
    }

    #[test]
    fn is_installed_false_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        assert!(!paths.is_installed());
    }

    #[test]
    fn is_installed_true_when_file_exists() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.service_file, "test").unwrap();
        assert!(paths.is_installed());
    }

    #[test]
    fn ensure_config_returns_explicit_path() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        let config_file = tmp.path().join("custom.yaml");
        fs::write(&config_file, "port: 3721\n").unwrap();
        let result = ensure_config(&paths, Some(config_file.to_str().unwrap())).unwrap();
        assert_eq!(result, config_file.to_string_lossy());
    }

    #[test]
    fn ensure_config_rejects_nonexistent_explicit_path() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        let result = ensure_config(&paths, Some("/nonexistent/config.yaml"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn ensure_config_returns_existing_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.default_config, "existing content").unwrap();
        let result = ensure_config(&paths, None).unwrap();
        assert_eq!(result, paths.default_config.to_string_lossy());
        assert_eq!(fs::read_to_string(&paths.default_config).unwrap(), "existing content");
    }

    #[test]
    fn ensure_config_creates_default_with_sqlite() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        let result = ensure_config(&paths, None).unwrap();
        assert_eq!(result, paths.default_config.to_string_lossy());
        let content = fs::read_to_string(&paths.default_config).unwrap();
        assert!(content.contains("port: 3721"));
        assert!(content.contains("provider: sqlite"));
        assert!(content.contains(&paths.default_db.to_string_lossy().to_string()));
    }

    #[test]
    fn write_and_read_state_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 8080).unwrap();
        assert_eq!(read_state_port(&paths), Some(8080));
    }

    #[test]
    fn read_state_port_returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        assert_eq!(read_state_port(&paths), None);
    }

    #[test]
    fn read_state_port_returns_none_for_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.state_file, "not json").unwrap();
        assert_eq!(read_state_port(&paths), None);
    }

    #[test]
    fn delete_state_removes_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 3721).unwrap();
        assert!(paths.state_file.exists());
        delete_state(&paths);
        assert!(!paths.state_file.exists());
    }

    #[test]
    fn delete_state_noop_when_missing() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        delete_state(&paths);
    }

    #[test]
    fn get_port_from_state_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 9090).unwrap();
        assert_eq!(get_port(&paths), 9090);
    }

    #[test]
    fn get_port_from_config_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.default_config, "port: 4000\n").unwrap();
        assert_eq!(get_port(&paths), 4000);
    }

    #[test]
    fn get_port_state_takes_precedence_over_config() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 9090).unwrap();
        fs::write(&paths.default_config, "port: 4000\n").unwrap();
        assert_eq!(get_port(&paths), 9090);
    }

    #[test]
    fn get_port_defaults_to_3721() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        assert_eq!(get_port(&paths), 3721);
    }

    #[test]
    fn get_port_ignores_invalid_config() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.default_config, "port: not_a_number\n").unwrap();
        assert_eq!(get_port(&paths), 3721);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_paths_are_correct() {
        let paths = ServicePaths::new().unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(paths.service_file, home.join("Library/LaunchAgents/com.taskcast.daemon.plist"));
        assert_eq!(paths.stdout_log.unwrap(), home.join("Library/Application Support/taskcast/taskcast.log"));
        assert_eq!(paths.stderr_log.unwrap(), home.join("Library/Application Support/taskcast/taskcast.err.log"));
        assert!(paths.log_dir.is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_paths_are_correct() {
        let paths = ServicePaths::new().unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(paths.service_file, home.join(".config/systemd/user/taskcast.service"));
        assert!(paths.log_dir.is_none());
        assert!(paths.stdout_log.is_none());
        assert!(paths.stderr_log.is_none());
    }
}
