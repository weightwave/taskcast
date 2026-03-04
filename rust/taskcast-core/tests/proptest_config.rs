use proptest::prelude::*;
use taskcast_core::config::{interpolate_env_vars, parse_config, ConfigFormat};
use ulid::Ulid;

proptest! {
    /// Strings without ${...} are returned unchanged
    #[test]
    fn interpolate_no_vars_is_identity(s in "[a-zA-Z0-9 _.:/-]{0,100}") {
        prop_assume!(!s.contains("${"));
        let result = interpolate_env_vars(&s);
        prop_assert_eq!(result, s);
    }

    /// Single env var reference is replaced when var is set
    #[test]
    fn interpolate_replaces_set_var(
        prefix in "[a-z]{0,10}",
        suffix in "[a-z]{0,10}",
        value in "[a-zA-Z0-9]{1,20}",
    ) {
        let var_name = format!("TASKCAST_PROPTEST_{}", Ulid::new());
        let var_name_safe = var_name.replace('-', "_");
        unsafe { std::env::set_var(&var_name_safe, &value); }
        let input = format!("{}${{{}}}{}", prefix, var_name_safe, suffix);
        let result = interpolate_env_vars(&input);
        unsafe { std::env::remove_var(&var_name_safe); }
        prop_assert_eq!(result, format!("{}{}{}", prefix, value, suffix));
    }

    /// Unset env vars are kept as the original ${VAR_NAME} placeholder
    #[test]
    fn interpolate_unset_var_kept_as_placeholder(
        prefix in "[a-z]{0,10}",
        suffix in "[a-z]{0,10}",
    ) {
        let var_name = format!("TASKCAST_PROPTEST_UNSET_{}", Ulid::new());
        let input = format!("{}${{{}}}{}", prefix, var_name, suffix);
        let result = interpolate_env_vars(&input);
        prop_assert_eq!(result, format!("{}${{{}}}{}", prefix, var_name, suffix));
    }

    /// Port values within u16 range parse successfully
    #[test]
    fn parse_config_valid_port(port in 1u16..=65535u16) {
        let json = format!(r#"{{"port": {}}}"#, port);
        let config = parse_config(&json, ConfigFormat::Json).unwrap();
        prop_assert_eq!(config.port, Some(port));
    }

    /// Port as string is coerced to number
    #[test]
    fn parse_config_string_port_coerced(port in 1u16..=65535u16) {
        let json = format!(r#"{{"port": "{}"}}"#, port);
        let config = parse_config(&json, ConfigFormat::Json).unwrap();
        prop_assert_eq!(config.port, Some(port));
    }

    /// Empty JSON object parses to default config
    #[test]
    fn parse_config_empty_object_always_succeeds(_ in 0u8..1u8) {
        let config = parse_config("{}", ConfigFormat::Json).unwrap();
        prop_assert!(config.port.is_none());
    }
}