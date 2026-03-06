# Embed Playground into Rust Binary via rust-embed

**Date:** 2026-03-06
**Status:** Approved

## Problem

The Rust CLI's `--playground` flag requires the playground dist directory to exist on the filesystem. The current 3-tier lookup (`TASKCAST_PLAYGROUND_DIR` env var, `../share/taskcast/playground/` relative to exe, `packages/playground/dist/` relative to cwd) fails for users who download the standalone binary from GitHub Releases. The feature is effectively unusable for most Rust binary users.

## Solution

Use `rust-embed` to embed the playground dist files (192KB total) directly into the compiled binary at build time. This gives users a true single-binary experience with zero filesystem dependencies for the playground.

## Design

### Rust Changes (`rust/taskcast-cli`)

**Cargo.toml:** Add `rust-embed = "8"`. Remove `tower-http` (fs feature) and `tower` since `ServeDir`/`ServeFile` are no longer needed.

**main.rs:**

1. Define embedded asset struct:
   ```rust
   #[derive(rust_embed::RustEmbed)]
   #[folder = "../../packages/playground/dist"]
   struct PlaygroundAssets;
   ```

2. Replace `resolve_playground_dir()` + `nest_playground()` (filesystem-based `ServeDir`) with a new Axum handler that serves from embedded assets:
   - GET `/_playground/*path` -> look up in `PlaygroundAssets::get(path)`
   - Set `Content-Type` based on file extension (html, js, css)
   - SPA fallback: unknown paths return `index.html`
   - Return 404 if asset not found (shouldn't happen with SPA fallback)

3. `--playground` remains an opt-in flag. `playground` subcommand also uses embedded assets.

4. Remove `resolve_playground_dir()`, `TASKCAST_PLAYGROUND_DIR` env var support, and all filesystem lookup logic.

### Dev Experience

`rust-embed` in debug mode reads files from disk at runtime (not embedded), so playground changes don't require Rust recompilation during development. Only release builds embed files.

### CI Changes

All Rust CI jobs that compile `taskcast-cli` need the playground dist to exist before `cargo build`. Add these steps before Rust compilation:

**rust.yml** (check, test, coverage, build jobs):
```yaml
- uses: actions/setup-node@v4
  with:
    node-version: '22'
- uses: pnpm/action-setup@v4
- run: pnpm install
- run: pnpm --filter @taskcast/playground build
```

**release.yml** (`rust-build` job): Already has Node.js setup. Add playground build before `cargo build`.

**Dockerfile** (`rust/Dockerfile`): Add a Node.js stage to build playground, then copy dist into build context before Rust compilation.

### What Doesn't Change

- TypeScript CLI (already uses `createRequire` to resolve playground package)
- `--playground` / `playground` CLI interface
- Playground source code
- Binary size impact: ~200KB increase (negligible vs current ~15MB binary)
