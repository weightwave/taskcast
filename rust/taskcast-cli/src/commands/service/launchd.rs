use super::manager::{ServiceInstallOptions, ServiceManager, ServiceStatus};

pub struct LaunchdServiceManager;

impl ServiceManager for LaunchdServiceManager {
    fn install(&self, _opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>> {
        todo!()
    }

    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> {
        todo!()
    }

    fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        todo!()
    }

    fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        todo!()
    }

    fn restart(&self) -> Result<(), Box<dyn std::error::Error>> {
        todo!()
    }

    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>> {
        todo!()
    }
}
