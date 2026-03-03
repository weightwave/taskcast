use proptest::prelude::*;
use taskcast_core::matches_type;

proptest! {
    /// None pattern matches any type string
    #[test]
    fn none_pattern_matches_everything(event_type in "[a-z.]{1,30}") {
        prop_assert!(matches_type(&event_type, None));
    }

    /// Wildcard "*" matches any type string
    #[test]
    fn star_pattern_matches_everything(event_type in "[a-z.]{1,30}") {
        prop_assert!(matches_type(&event_type, Some(&["*".to_string()])));
    }

    /// Empty pattern list matches nothing
    #[test]
    fn empty_pattern_matches_nothing(event_type in "[a-z.]{1,30}") {
        let empty: &[String] = &[];
        prop_assert!(!matches_type(&event_type, Some(empty)));
    }

    /// Exact match always works
    #[test]
    fn exact_match_always_succeeds(event_type in "[a-z]{1,10}(\\.[a-z]{1,10}){0,3}") {
        prop_assert!(matches_type(&event_type, Some(&[event_type.clone()])));
    }

    /// prefix.* matches prefix.anything but not prefix alone
    #[test]
    fn prefix_wildcard_matches_children(
        prefix in "[a-z]{1,10}",
        suffix in "[a-z]{1,10}",
    ) {
        let pattern = format!("{}.*", prefix);
        let event_type = format!("{}.{}", prefix, suffix);
        prop_assert!(matches_type(&event_type, Some(&[pattern.clone()])));
        // prefix alone should NOT match prefix.*
        prop_assert!(!matches_type(&prefix, Some(&[pattern])));
    }
}