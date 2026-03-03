#!/usr/bin/env bash
set -euo pipefail

# Usage: update-homebrew.sh <version> <artifacts-dir> <tap-repo-dir>
# Example: update-homebrew.sh 0.1.2 /tmp/binaries /tmp/homebrew-tap

VERSION="${1:?Usage: update-homebrew.sh <version> <artifacts-dir> <tap-repo-dir>}"
ARTIFACTS_DIR="${2:?Missing artifacts directory}"
TAP_DIR="${3:?Missing tap repo directory}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEMPLATE="$SCRIPT_DIR/homebrew-formula.rb.template"

if [ ! -f "$TEMPLATE" ]; then
  echo "ERROR: Template not found at $TEMPLATE" >&2
  exit 1
fi

sha256_of() {
  shasum -a 256 "$1" | awk '{print $1}'
}

SHA_MACOS_ARM64=$(sha256_of "$ARTIFACTS_DIR/taskcast-v${VERSION}-aarch64-apple-darwin.tar.gz")
SHA_MACOS_X64=$(sha256_of "$ARTIFACTS_DIR/taskcast-v${VERSION}-x86_64-apple-darwin.tar.gz")
SHA_LINUX_X64=$(sha256_of "$ARTIFACTS_DIR/taskcast-v${VERSION}-x86_64-unknown-linux-gnu.tar.gz")
SHA_LINUX_ARM64=$(sha256_of "$ARTIFACTS_DIR/taskcast-v${VERSION}-aarch64-unknown-linux-gnu.tar.gz")

echo "Version: $VERSION"
echo "SHA256 macOS ARM64: $SHA_MACOS_ARM64"
echo "SHA256 macOS x64:   $SHA_MACOS_X64"
echo "SHA256 Linux x64:   $SHA_LINUX_X64"
echo "SHA256 Linux ARM64: $SHA_LINUX_ARM64"

mkdir -p "$TAP_DIR/Formula"

sed \
  -e "s/__VERSION__/$VERSION/g" \
  -e "s/__SHA256_MACOS_ARM64__/$SHA_MACOS_ARM64/g" \
  -e "s/__SHA256_MACOS_X64__/$SHA_MACOS_X64/g" \
  -e "s/__SHA256_LINUX_X64__/$SHA_LINUX_X64/g" \
  -e "s/__SHA256_LINUX_ARM64__/$SHA_LINUX_ARM64/g" \
  "$TEMPLATE" > "$TAP_DIR/Formula/taskcast.rb"

echo "Formula written to $TAP_DIR/Formula/taskcast.rb"
