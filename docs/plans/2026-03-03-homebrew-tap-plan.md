# Homebrew Tap Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable `brew tap weightwave/tap && brew install taskcast` to install the Rust CLI from precompiled binaries.

**Architecture:** A separate `weightwave/homebrew-tap` repo holds a single Formula. A template in this repo (`scripts/homebrew-formula.rb.template`) is rendered with version + SHA256 values by a new CI job that pushes to the tap repo after every release.

**Tech Stack:** Homebrew Ruby formula, GitHub Actions, bash scripting

---

### Task 1: Create the Formula Template

**Files:**
- Create: `scripts/homebrew-formula.rb.template`

**Step 1: Create the template file**

```ruby
class Taskcast < Formula
  desc "Unified long-lifecycle task tracking service for LLM streaming and async workloads"
  homepage "https://github.com/weightwave/taskcast"
  version "__VERSION__"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/weightwave/taskcast/releases/download/v#{version}/taskcast-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA256_MACOS_ARM64__"
    end
    if Hardware::CPU.intel?
      url "https://github.com/weightwave/taskcast/releases/download/v#{version}/taskcast-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA256_MACOS_X64__"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/weightwave/taskcast/releases/download/v#{version}/taskcast-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "__SHA256_LINUX_ARM64__"
    end
    if Hardware::CPU.intel?
      url "https://github.com/weightwave/taskcast/releases/download/v#{version}/taskcast-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "__SHA256_LINUX_X64__"
    end
  end

  def install
    bin.install "taskcast"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/taskcast --version")
  end
end
```

**Step 2: Commit**

```bash
git add scripts/homebrew-formula.rb.template
git commit -m "feat: add Homebrew formula template"
```

---

### Task 2: Create the Update Script

**Files:**
- Create: `scripts/update-homebrew.sh`

This script takes a version string, computes SHA256 from local archive files, renders the template, and commits to the tap repo.

**Step 1: Create the script**

```bash
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
```

**Step 2: Make it executable and commit**

```bash
chmod +x scripts/update-homebrew.sh
git add scripts/update-homebrew.sh
git commit -m "feat: add Homebrew formula update script"
```

---

### Task 3: Add the `update-homebrew` Job to release.yml

**Files:**
- Modify: `.github/workflows/release.yml`

**Step 1: Add the new job at the end of the file**

Append after the `attach-binaries` job:

```yaml
  # ── Update Homebrew tap ────────────────────────────────────────────────────
  update-homebrew:
    needs: [get-version, rust-build]
    runs-on: ubuntu-latest
    env:
      VERSION: ${{ needs.get-version.outputs.version }}
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Download all binary artifacts
        uses: actions/download-artifact@v4
        with:
          pattern: binary-*
          path: /tmp/binaries
          merge-multiple: true

      - name: Clone homebrew-tap
        run: |
          git clone https://x-access-token:${{ secrets.HOMEBREW_TAP_TOKEN }}@github.com/weightwave/homebrew-tap.git /tmp/homebrew-tap

      - name: Update formula
        run: bash scripts/update-homebrew.sh "$VERSION" /tmp/binaries /tmp/homebrew-tap

      - name: Push to homebrew-tap
        working-directory: /tmp/homebrew-tap
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add Formula/taskcast.rb
          git commit -m "taskcast $VERSION"
          git push
```

**Step 2: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add Homebrew tap update job to release workflow"
```

---

### Task 4: Create the `weightwave/homebrew-tap` Repository

This task is done manually on GitHub (not code changes).

**Step 1: Create the repository**

Go to GitHub → `weightwave` org → New repository:
- Name: `homebrew-tap`
- Public
- Initialize with README

**Step 2: Create a PAT**

Create a fine-grained GitHub PAT with:
- Repository access: `weightwave/homebrew-tap` only
- Permissions: Contents (Read and write)

**Step 3: Add the secret**

In `weightwave/taskcast` → Settings → Secrets → Actions:
- Name: `HOMEBREW_TAP_TOKEN`
- Value: the PAT from step 2

---

### Task 5: Bootstrap the Initial Formula (Optional)

If you want the formula available before the next release, manually run the update script against the current release (v0.1.2).

**Step 1: Download current release archives**

```bash
mkdir -p /tmp/binaries
cd /tmp/binaries
for target in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
  gh release download v0.1.2 -R weightwave/taskcast -p "taskcast-v0.1.2-${target}.tar.gz"
done
```

**Step 2: Clone tap and run update script**

```bash
gh repo clone weightwave/homebrew-tap /tmp/homebrew-tap
cd /Users/winrey/Projects/taskcast
bash scripts/update-homebrew.sh 0.1.2 /tmp/binaries /tmp/homebrew-tap
```

**Step 3: Push**

```bash
cd /tmp/homebrew-tap
git add Formula/taskcast.rb
git commit -m "taskcast 0.1.2"
git push
```

**Step 4: Test installation**

```bash
brew tap weightwave/tap
brew install taskcast
taskcast --version
```
