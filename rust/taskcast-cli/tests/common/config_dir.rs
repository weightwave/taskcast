use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use tempfile::TempDir;

const CONFIG_DIR_ENV: &str = "TASKCAST_CONFIG_DIR";
static CONFIG_DIR_LOCK: Mutex<()> = Mutex::new(());

pub struct IsolatedConfigDir {
    dir: TempDir,
    previous: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl IsolatedConfigDir {
    pub fn new() -> Self {
        let lock = CONFIG_DIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os(CONFIG_DIR_ENV);
        let dir = TempDir::new().expect("create isolated Taskcast config directory");
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        Self {
            dir,
            previous,
            _lock: lock,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for IsolatedConfigDir {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(CONFIG_DIR_ENV, value),
            None => std::env::remove_var(CONFIG_DIR_ENV),
        }
    }
}
