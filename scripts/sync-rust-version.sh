#!/usr/bin/env bash
# sync-rust-version.sh
# Reads the npm @taskcast/cli version and patches all Rust Cargo.toml versions to match.
# Runs in CI before building Rust binaries. Changes are never committed.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

VERSION=$(cd "$ROOT" && node -p "require('./packages/cli/package.json').version")

if [ -z "$VERSION" ]; then
  echo "ERROR: Could not read version from packages/cli/package.json" >&2
  exit 1
fi

CRATES=(
  "$ROOT/rust/taskcast-cli/Cargo.toml"
  "$ROOT/rust/taskcast-core/Cargo.toml"
  "$ROOT/rust/taskcast-server/Cargo.toml"
  "$ROOT/rust/taskcast-redis/Cargo.toml"
  "$ROOT/rust/taskcast-postgres/Cargo.toml"
)

for CRATE in "${CRATES[@]}"; do
  # Replace only the first 'version = "..."' line (the [package] version).
  # Uses awk for portability across macOS (BSD sed) and Linux (GNU sed).
  awk -v ver="$VERSION" '
    !done && /^version = ".*"/ { print "version = \"" ver "\""; done=1; next }
    { print }
  ' "$CRATE" > "$CRATE.tmp" && mv "$CRATE.tmp" "$CRATE"
done

echo "$VERSION"
