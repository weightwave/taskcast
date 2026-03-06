# Homebrew Tap for Taskcast Rust CLI

## Goal

Allow users to install the Taskcast Rust CLI via Homebrew:

```bash
brew tap weightwave/tap
brew install taskcast
```

## Approach

**Precompiled binary distribution** via a dedicated Homebrew tap repository (`weightwave/homebrew-tap`), with CI automation to update the formula on every release.

## Tap Repository

Repository: `weightwave/homebrew-tap` on GitHub.

```
homebrew-tap/
├── Formula/
│   └── taskcast.rb
└── README.md
```

## Formula

The formula downloads precompiled binaries from GitHub Releases. It supports four platforms:

| Platform | Target Triple | Archive Format |
|----------|---------------|----------------|
| macOS ARM64 | `aarch64-apple-darwin` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `.tar.gz` |

The formula uses `on_macos`/`on_linux` blocks with `Hardware::CPU.arm?`/`Hardware::CPU.intel?` to select the correct binary. The archive contains a single `taskcast` executable which is installed to `bin/`.

## CI Automation

A new `update-homebrew` job in `.github/workflows/release.yml` runs after `attach-binaries`:

1. Downloads all 4 platform archive artifacts (reuses existing `binary-*` artifacts from `rust-build`)
2. Computes SHA256 for each archive
3. Generates the formula from a template (`scripts/homebrew-formula.rb.template`)
4. Clones `weightwave/homebrew-tap`, commits the updated formula, and pushes

### Template

A Ruby template file at `scripts/homebrew-formula.rb.template` with placeholders:

- `__VERSION__`
- `__SHA256_MACOS_ARM64__`
- `__SHA256_MACOS_X64__`
- `__SHA256_LINUX_X64__`
- `__SHA256_LINUX_ARM64__`

### Required Secret

`HOMEBREW_TAP_TOKEN` — a GitHub PAT with write access to `weightwave/homebrew-tap`.

## Job Dependency Graph

```
release → get-version → rust-build → attach-binaries → update-homebrew
                      → docker-build → docker-merge
```

## User Experience

```bash
# Install
brew tap weightwave/tap
brew install taskcast

# Update
brew upgrade taskcast

# Verify
taskcast --version
```
