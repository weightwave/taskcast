use std::ffi::OsString;
use std::path::PathBuf;

pub const TASKCAST_CONFIG_DIR_ENV: &str = "TASKCAST_CONFIG_DIR";
const CONFIG_DIR_ERROR: &str = "cannot determine Taskcast config directory";

fn resolve_taskcast_config_dir(
    override_dir: Option<OsString>,
    home_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, &'static str> {
    match override_dir {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => home_dir()
            .map(|home| home.join(".taskcast"))
            .ok_or(CONFIG_DIR_ERROR),
    }
}

pub fn taskcast_config_dir() -> Result<PathBuf, &'static str> {
    resolve_taskcast_config_dir(std::env::var_os(TASKCAST_CONFIG_DIR_ENV), dirs::home_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn non_empty_override_wins() {
        let override_dir = OsString::from("isolated-config");
        let result = resolve_taskcast_config_dir(Some(override_dir), || {
            panic!("home lookup must not run when an override is present")
        });
        assert_eq!(result.unwrap(), PathBuf::from("isolated-config"));
    }

    #[test]
    fn relative_override_is_preserved() {
        let result = resolve_taskcast_config_dir(Some(OsString::from("relative/config")), || {
            Some(PathBuf::from("unused-home"))
        });
        assert_eq!(result.unwrap(), PathBuf::from("relative/config"));
    }

    #[test]
    fn empty_override_falls_back_to_dot_taskcast() {
        let home = PathBuf::from("fake-home");
        let result = resolve_taskcast_config_dir(Some(OsString::new()), || Some(home.clone()));
        assert_eq!(result.unwrap(), home.join(".taskcast"));
    }

    #[test]
    fn absent_override_falls_back_to_dot_taskcast() {
        let home = PathBuf::from("fake-home");
        let result = resolve_taskcast_config_dir(None, || Some(home.clone()));
        assert_eq!(result.unwrap(), home.join(".taskcast"));
    }

    #[test]
    fn missing_override_and_home_returns_clear_error() {
        let result = resolve_taskcast_config_dir(None, || None);
        assert_eq!(result.unwrap_err(), CONFIG_DIR_ERROR);
    }

    #[cfg(unix)]
    #[test]
    fn unix_non_unicode_override_is_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![b'c', b'f', b'g', 0xff]);
        let result = resolve_taskcast_config_dir(Some(raw.clone()), || None);
        assert_eq!(result.unwrap(), PathBuf::from(raw));
    }

    #[cfg(windows)]
    #[test]
    fn windows_non_unicode_override_is_preserved() {
        use std::os::windows::ffi::OsStringExt;
        let raw = OsString::from_wide(&[b'c' as u16, b'f' as u16, b'g' as u16, 0xd800]);
        let result = resolve_taskcast_config_dir(Some(raw.clone()), || None);
        assert_eq!(result.unwrap(), PathBuf::from(raw));
    }
}
