use std::fmt;

pub struct ServiceInstallOptions {
    pub port: u16,
    pub config: Option<String>,
    pub storage: Option<String>,
    pub db_path: Option<String>,
    pub exec_path: String,
}

#[derive(Debug, PartialEq)]
pub enum ServiceStatus {
    Running { pid: u32 },
    Stopped,
    NotInstalled,
}

impl fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceStatus::Running { pid } => write!(f, "running (pid {pid})"),
            ServiceStatus::Stopped => write!(f, "stopped"),
            ServiceStatus::NotInstalled => write!(f, "not installed"),
        }
    }
}

pub trait ServiceManager {
    fn install(&self, opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>>;
    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn start(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn stop(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn restart(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>>;
}

#[allow(clippy::needless_return)]
pub fn create_service_manager() -> Result<Box<dyn ServiceManager>, Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    {
        return Ok(Box::new(super::launchd::LaunchdServiceManager));
    }
    #[cfg(target_os = "linux")]
    {
        return Ok(Box::new(super::systemd::SystemdServiceManager));
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("Service management is not supported on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_display_running() {
        let status = ServiceStatus::Running { pid: 12345 };
        assert_eq!(status.to_string(), "running (pid 12345)");
    }

    #[test]
    fn status_display_stopped() {
        let status = ServiceStatus::Stopped;
        assert_eq!(status.to_string(), "stopped");
    }

    #[test]
    fn status_display_not_installed() {
        let status = ServiceStatus::NotInstalled;
        assert_eq!(status.to_string(), "not installed");
    }

    #[test]
    fn status_equality() {
        // eq cases
        assert_eq!(ServiceStatus::Running { pid: 1 }, ServiceStatus::Running { pid: 1 });
        assert_eq!(ServiceStatus::Stopped, ServiceStatus::Stopped);
        assert_eq!(ServiceStatus::NotInstalled, ServiceStatus::NotInstalled);

        // ne cases
        assert_ne!(ServiceStatus::Running { pid: 1 }, ServiceStatus::Running { pid: 2 });
        assert_ne!(ServiceStatus::Running { pid: 1 }, ServiceStatus::Stopped);
        assert_ne!(ServiceStatus::Stopped, ServiceStatus::NotInstalled);
    }

    #[test]
    fn install_options_fields() {
        let opts = ServiceInstallOptions {
            port: 8080,
            config: Some("/etc/taskcast/config.toml".to_string()),
            storage: Some("redis".to_string()),
            db_path: Some("/var/lib/taskcast/db.sqlite".to_string()),
            exec_path: "/usr/local/bin/taskcast".to_string(),
        };

        assert_eq!(opts.port, 8080);
        assert_eq!(opts.config.as_deref(), Some("/etc/taskcast/config.toml"));
        assert_eq!(opts.storage.as_deref(), Some("redis"));
        assert_eq!(opts.db_path.as_deref(), Some("/var/lib/taskcast/db.sqlite"));
        assert_eq!(opts.exec_path, "/usr/local/bin/taskcast");
    }

    #[test]
    fn install_options_optional_fields_none() {
        let opts = ServiceInstallOptions {
            port: 3000,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/bin/taskcast-rs".to_string(),
        };

        assert_eq!(opts.port, 3000);
        assert!(opts.config.is_none());
        assert!(opts.storage.is_none());
        assert!(opts.db_path.is_none());
        assert_eq!(opts.exec_path, "/usr/bin/taskcast-rs");
    }

    #[test]
    fn create_service_manager_on_current_platform() {
        let result = create_service_manager();

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(result.is_ok(), "Expected Ok on macOS/Linux, got: {:?}", result.err());

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        assert!(result.is_err(), "Expected Err on unsupported platform");
    }
}
