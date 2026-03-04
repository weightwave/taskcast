# Rust Test Coverage Improvements

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add coverage reporting to CI, fill test gaps in auth/error paths, and add property-based testing for core logic.

**Architecture:** cargo-llvm-cov for coverage reporting (more accurate than tarpaulin for async code). Unit tests for auth pure functions. proptest for property-based testing of config parsing and filter logic.

**Tech Stack:** cargo-llvm-cov, proptest, jsonwebtoken, testcontainers

---

## Task 1: Add cargo-llvm-cov to CI

**Files:**
- Modify: `.github/workflows/rust.yml`

**Step 1: Add coverage job to CI workflow**

Add a new `coverage` job after the existing `test` job in `.github/workflows/rust.yml`:

```yaml
  coverage:
    name: Coverage
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: rust
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: llvm-tools-preview
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: rust
      - name: Install cargo-llvm-cov
        uses: taiki-e/install-action@cargo-llvm-cov
      - name: Generate coverage
        run: cargo llvm-cov --workspace --lcov --output-path lcov.info
      - name: Upload coverage to Codecov
        uses: codecov/codecov-action@v4
        with:
          files: rust/lcov.info
          flags: rust
          fail_ci_if_error: false
        env:
          CODECOV_TOKEN: ${{ secrets.CODECOV_TOKEN }}
```

**Step 2: Verify the workflow YAML is valid**

Run: `cd /Users/winrey/Projects/taskcast && python3 -c "import yaml; yaml.safe_load(open('.github/workflows/rust.yml'))"`
Expected: No errors

**Step 3: Commit**

```bash
git add .github/workflows/rust.yml
git commit -m "ci: add cargo-llvm-cov coverage reporting to Rust workflow"
```

---

## Task 2: Auth module unit tests — check_scope

**Files:**
- Modify: `rust/taskcast-server/src/auth.rs` (add `#[cfg(test)] mod tests`)

**Step 1: Write failing tests for check_scope**

Add at the bottom of `rust/taskcast-server/src/auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use taskcast_core::PermissionScope;

    fn ctx_with_scope(scope: Vec<PermissionScope>) -> AuthContext {
        AuthContext {
            sub: Some("user1".to_string()),
            task_ids: TaskIdAccess::All,
            scope,
        }
    }

    fn ctx_with_task_ids(ids: Vec<&str>, scope: Vec<PermissionScope>) -> AuthContext {
        AuthContext {
            sub: Some("user1".to_string()),
            task_ids: TaskIdAccess::List(ids.into_iter().map(String::from).collect()),
            scope,
        }
    }

    // ─── check_scope tests ──────────────────────────────────────────────────

    #[test]
    fn check_scope_all_grants_any_permission() {
        let ctx = ctx_with_scope(vec![PermissionScope::All]);
        assert!(check_scope(&ctx, PermissionScope::TaskCreate, None));
        assert!(check_scope(&ctx, PermissionScope::EventPublish, None));
        assert!(check_scope(&ctx, PermissionScope::EventSubscribe, None));
    }

    #[test]
    fn check_scope_specific_scope_grants_matching() {
        let ctx = ctx_with_scope(vec![PermissionScope::TaskCreate]);
        assert!(check_scope(&ctx, PermissionScope::TaskCreate, None));
    }

    #[test]
    fn check_scope_specific_scope_denies_non_matching() {
        let ctx = ctx_with_scope(vec![PermissionScope::TaskCreate]);
        assert!(!check_scope(&ctx, PermissionScope::EventPublish, None));
    }

    #[test]
    fn check_scope_empty_scope_denies_all() {
        let ctx = ctx_with_scope(vec![]);
        assert!(!check_scope(&ctx, PermissionScope::TaskCreate, None));
    }

    #[test]
    fn check_scope_task_id_access_all_allows_any_task() {
        let ctx = ctx_with_scope(vec![PermissionScope::All]);
        assert!(check_scope(&ctx, PermissionScope::TaskCreate, Some("any-task-id")));
    }

    #[test]
    fn check_scope_task_id_list_allows_matching_task() {
        let ctx = ctx_with_task_ids(vec!["task-1", "task-2"], vec![PermissionScope::All]);
        assert!(check_scope(&ctx, PermissionScope::TaskCreate, Some("task-1")));
        assert!(check_scope(&ctx, PermissionScope::TaskCreate, Some("task-2")));
    }

    #[test]
    fn check_scope_task_id_list_denies_non_matching_task() {
        let ctx = ctx_with_task_ids(vec!["task-1"], vec![PermissionScope::All]);
        assert!(!check_scope(&ctx, PermissionScope::TaskCreate, Some("task-999")));
    }

    #[test]
    fn check_scope_task_id_check_skipped_when_no_task_id() {
        let ctx = ctx_with_task_ids(vec!["task-1"], vec![PermissionScope::All]);
        assert!(check_scope(&ctx, PermissionScope::TaskCreate, None));
    }

    #[test]
    fn check_scope_both_scope_and_task_id_must_match() {
        let ctx = ctx_with_task_ids(vec!["task-1"], vec![PermissionScope::TaskCreate]);
        // Right scope, right task
        assert!(check_scope(&ctx, PermissionScope::TaskCreate, Some("task-1")));
        // Right scope, wrong task
        assert!(!check_scope(&ctx, PermissionScope::TaskCreate, Some("task-2")));
        // Wrong scope, right task
        assert!(!check_scope(&ctx, PermissionScope::EventPublish, Some("task-1")));
    }
}
```

**Step 2: Run tests to verify they pass**

Run: `cd /Users/winrey/Projects/taskcast/rust && cargo test -p taskcast-server -- auth::tests --nocapture`
Expected: All 8 tests PASS (these test existing behavior, not new code)

**Step 3: Commit**

```bash
git add rust/taskcast-server/src/auth.rs
git commit -m "test(server): add unit tests for check_scope"
```

---

## Task 3: Auth module unit tests — decode_jwt

**Files:**
- Modify: `rust/taskcast-server/src/auth.rs` (extend tests module)

**Step 1: Add jsonwebtoken to dev-dependencies**

Add to `rust/taskcast-server/Cargo.toml` under `[dev-dependencies]`:

```toml
jsonwebtoken = "9"
```

Note: `jsonwebtoken` is already a normal dependency, but we need it in tests too. Since it's already in `[dependencies]`, we can use it directly in `#[cfg(test)]` — no extra dev-dep needed. Skip this step.

**Step 2: Write decode_jwt tests**

Append to the `tests` module in `rust/taskcast-server/src/auth.rs`:

```rust
    // ─── decode_jwt tests ───────────────────────────────────────────────────

    use jsonwebtoken::{encode, EncodingKey, Header};

    const TEST_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

    fn hs256_config() -> JwtConfig {
        JwtConfig {
            algorithm: Algorithm::HS256,
            secret: Some(TEST_SECRET.to_string()),
            public_key: None,
            issuer: None,
            audience: None,
        }
    }

    fn make_token(claims: &JwtClaims) -> String {
        encode(
            &Header::default(),
            claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap()
    }

    fn base_claims() -> JwtClaims {
        JwtClaims {
            sub: Some("user1".to_string()),
            task_ids: None,
            scope: Some(vec![PermissionScope::All]),
            iss: None,
            aud: None,
            exp: Some((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()) + 3600),
            iat: None,
        }
    }

    #[test]
    fn decode_jwt_valid_token_extracts_sub() {
        let claims = base_claims();
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).unwrap();
        assert_eq!(ctx.sub, Some("user1".to_string()));
    }

    #[test]
    fn decode_jwt_valid_token_extracts_scope() {
        let claims = base_claims();
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).unwrap();
        assert_eq!(ctx.scope, vec![PermissionScope::All]);
    }

    #[test]
    fn decode_jwt_no_scope_defaults_to_empty() {
        let mut claims = base_claims();
        claims.scope = None;
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).unwrap();
        assert!(ctx.scope.is_empty());
    }

    #[test]
    fn decode_jwt_wildcard_task_ids_maps_to_all() {
        let mut claims = base_claims();
        claims.task_ids = Some(TaskIdsClaim::Wildcard("*".to_string()));
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).unwrap();
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
    }

    #[test]
    fn decode_jwt_non_star_wildcard_maps_to_all() {
        let mut claims = base_claims();
        claims.task_ids = Some(TaskIdsClaim::Wildcard("anything".to_string()));
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).unwrap();
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
    }

    #[test]
    fn decode_jwt_list_task_ids_preserved() {
        let mut claims = base_claims();
        claims.task_ids = Some(TaskIdsClaim::List(vec!["t1".to_string(), "t2".to_string()]));
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).unwrap();
        match ctx.task_ids {
            TaskIdAccess::List(ids) => assert_eq!(ids, vec!["t1", "t2"]),
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn decode_jwt_no_task_ids_defaults_to_all() {
        let claims = base_claims();
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).unwrap();
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
    }

    #[test]
    fn decode_jwt_expired_token_fails() {
        let mut claims = base_claims();
        claims.exp = Some(1000); // long expired
        let token = make_token(&claims);
        assert!(decode_jwt(&token, &hs256_config()).is_err());
    }

    #[test]
    fn decode_jwt_wrong_secret_fails() {
        let claims = base_claims();
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.secret = Some("wrong-secret-key-that-does-not-match".to_string());
        assert!(decode_jwt(&token, &config).is_err());
    }

    #[test]
    fn decode_jwt_no_key_configured_fails() {
        let claims = base_claims();
        let token = make_token(&claims);
        let config = JwtConfig {
            algorithm: Algorithm::HS256,
            secret: None,
            public_key: None,
            issuer: None,
            audience: None,
        };
        assert!(decode_jwt(&token, &config).is_err());
    }

    #[test]
    fn decode_jwt_issuer_validation_accepts_matching() {
        let mut claims = base_claims();
        claims.iss = Some("my-issuer".to_string());
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.issuer = Some("my-issuer".to_string());
        assert!(decode_jwt(&token, &config).is_ok());
    }

    #[test]
    fn decode_jwt_issuer_validation_rejects_wrong() {
        let mut claims = base_claims();
        claims.iss = Some("wrong-issuer".to_string());
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.issuer = Some("expected-issuer".to_string());
        assert!(decode_jwt(&token, &config).is_err());
    }

    #[test]
    fn decode_jwt_audience_validation_accepts_matching() {
        let mut claims = base_claims();
        claims.aud = Some(serde_json::json!("my-audience"));
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.audience = Some("my-audience".to_string());
        assert!(decode_jwt(&token, &config).is_ok());
    }

    #[test]
    fn decode_jwt_audience_validation_rejects_wrong() {
        let mut claims = base_claims();
        claims.aud = Some(serde_json::json!("wrong-audience"));
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.audience = Some("expected-audience".to_string());
        assert!(decode_jwt(&token, &config).is_err());
    }

    #[test]
    fn decode_jwt_garbage_token_fails() {
        assert!(decode_jwt("not-a-real-token", &hs256_config()).is_err());
    }

    #[test]
    fn decode_jwt_empty_token_fails() {
        assert!(decode_jwt("", &hs256_config()).is_err());
    }

    #[test]
    fn auth_context_open_has_all_access() {
        let ctx = AuthContext::open();
        assert!(ctx.sub.is_none());
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
        assert_eq!(ctx.scope, vec![PermissionScope::All]);
    }
```

**Step 3: Run tests**

Run: `cd /Users/winrey/Projects/taskcast/rust && cargo test -p taskcast-server -- auth::tests --nocapture`
Expected: All 25 tests PASS (8 from Task 2 + 17 new)

**Step 4: Commit**

```bash
git add rust/taskcast-server/src/auth.rs
git commit -m "test(server): add unit tests for decode_jwt and AuthContext"
```

---

## Task 4: Property-based tests for config parsing

**Files:**
- Modify: `rust/taskcast-core/Cargo.toml` (add proptest dev-dep)
- Create: `rust/taskcast-core/tests/proptest_config.rs`

**Step 1: Add proptest dev-dependency**

Add to `rust/taskcast-core/Cargo.toml` under `[dev-dependencies]`:

```toml
proptest = "1"
```

**Step 2: Write property-based tests for env var interpolation**

Create `rust/taskcast-core/tests/proptest_config.rs`:

```rust
use proptest::prelude::*;
use taskcast_core::config::{interpolate_env_vars, parse_config, ConfigFormat};

// ─── interpolate_env_vars ───────────────────────────────────────────────────

proptest! {
    /// Strings without ${...} are returned unchanged
    #[test]
    fn interpolate_no_vars_is_identity(s in "[a-zA-Z0-9 _.:/-]{0,100}") {
        // Ensure no ${...} pattern exists
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
        let var_name = format!("TASKCAST_PROPTEST_{}", ulid::Ulid::new());
        let var_name_safe = var_name.replace('-', "_");
        std::env::set_var(&var_name_safe, &value);
        let input = format!("{}${{{}}}{}", prefix, var_name_safe, suffix);
        let result = interpolate_env_vars(&input);
        std::env::remove_var(&var_name_safe);
        prop_assert_eq!(result, format!("{}{}{}", prefix, value, suffix));
    }

    /// Unset env vars are replaced with empty string
    #[test]
    fn interpolate_unset_var_becomes_empty(
        prefix in "[a-z]{0,10}",
        suffix in "[a-z]{0,10}",
    ) {
        let var_name = format!("TASKCAST_PROPTEST_UNSET_{}", ulid::Ulid::new());
        let input = format!("{}${{{}}}{}", prefix, var_name, suffix);
        let result = interpolate_env_vars(&input);
        prop_assert_eq!(result, format!("{}{}", prefix, suffix));
    }
}

// ─── parse_config roundtrip ─────────────────────────────────────────────────

proptest! {
    /// Port values within u16 range parse successfully
    #[test]
    fn parse_config_valid_port(port in 1u16..=65535u16) {
        let json = format!(r#"{{"port": {}}}"#, port);
        let config = parse_config(&json, ConfigFormat::Json).unwrap();
        prop_assert_eq!(config.port, Some(port as u64));
    }

    /// Port as string is coerced to number
    #[test]
    fn parse_config_string_port_coerced(port in 1u16..=65535u16) {
        let json = format!(r#"{{"port": "{}"}}"#, port);
        let config = parse_config(&json, ConfigFormat::Json).unwrap();
        prop_assert_eq!(config.port, Some(port as u64));
    }

    /// Empty JSON object parses to default config
    #[test]
    fn parse_config_empty_object_always_succeeds(_ in 0u8..1u8) {
        let config = parse_config("{}", ConfigFormat::Json).unwrap();
        prop_assert!(config.port.is_none());
    }
}
```

**Step 3: Run tests**

Run: `cd /Users/winrey/Projects/taskcast/rust && cargo test -p taskcast-core --test proptest_config --nocapture`
Expected: All property tests PASS (proptest runs 256 cases per test by default)

**Step 4: Commit**

```bash
git add rust/taskcast-core/Cargo.toml rust/taskcast-core/tests/proptest_config.rs
git commit -m "test(core): add property-based tests for config parsing"
```

---

## Task 5: Property-based tests for filter logic

**Files:**
- Create: `rust/taskcast-core/tests/proptest_filter.rs`

**Step 1: Write property-based tests for filter matching**

Create `rust/taskcast-core/tests/proptest_filter.rs`:

```rust
use proptest::prelude::*;
use taskcast_core::filter::matches_type;

// ─── matches_type ───────────────────────────────────────────────────────────

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
```

**Step 2: Run tests**

Run: `cd /Users/winrey/Projects/taskcast/rust && cargo test -p taskcast-core --test proptest_filter --nocapture`
Expected: All property tests PASS

**Step 3: Commit**

```bash
git add rust/taskcast-core/tests/proptest_filter.rs
git commit -m "test(core): add property-based tests for filter matching"
```

---

## Task 6: Verify full test suite passes

**Step 1: Run all Rust tests**

Run: `cd /Users/winrey/Projects/taskcast/rust && cargo test --workspace`
Expected: All tests pass (existing 253+ new tests)

**Step 2: Run clippy**

Run: `cd /Users/winrey/Projects/taskcast/rust && cargo clippy --workspace -- -D warnings`
Expected: No warnings

**Step 3: Commit any fixes needed, then done**

---

## Summary

| Task | Priority | Tests Added | Area |
|------|----------|-------------|------|
| 1 | High | — | CI coverage reporting |
| 2 | High | 8 | auth check_scope |
| 3 | High | 17 | auth decode_jwt |
| 4 | Medium | ~6 (×256 cases) | config proptest |
| 5 | Medium | ~5 (×256 cases) | filter proptest |
| 6 | — | — | Verification |

**Total new tests:** ~30 explicit + ~2800 property-based cases
