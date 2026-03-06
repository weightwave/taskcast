# Rust-Embed Playground Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Embed the playground frontend dist files into the Rust binary using `rust-embed` so the `--playground` flag works out of the box for standalone binary users.

**Architecture:** Replace filesystem-based static file serving (`tower-http::ServeDir`) with `rust-embed` compile-time embedding. An Axum handler serves files from the embedded assets with correct MIME types and SPA fallback. CI pipelines build the playground before compiling Rust.

**Tech Stack:** rust-embed 8, Axum 0.8, mime_guess

---

### Task 1: Update Cargo.toml Dependencies

**Files:**
- Modify: `rust/taskcast-cli/Cargo.toml`

**Step 1: Replace tower-http/tower with rust-embed and mime_guess**

Replace lines 23-24 of `rust/taskcast-cli/Cargo.toml`:

```toml
# Remove these two lines:
tower-http = { version = "0.6", features = ["fs"] }
tower = "0.5"

# Add these:
rust-embed = "8"
mime_guess = "2"
```

The full `[dependencies]` section becomes:
```toml
[dependencies]
taskcast-core = { path = "../taskcast-core" }
taskcast-server = { path = "../taskcast-server" }
taskcast-postgres = { path = "../taskcast-postgres" }
taskcast-redis = { path = "../taskcast-redis" }
taskcast-sqlite = { path = "../taskcast-sqlite" }
clap = { version = "4", features = ["derive"] }
tokio = { workspace = true }
serde_json = { workspace = true }
axum = "0.8"
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres"] }
redis = { version = "0.27", features = ["tokio-comp", "aio"] }
jsonwebtoken = "9"
rust-embed = "8"
mime_guess = "2"
```

**Step 2: Verify compilation**

Run: `cd rust && cargo check -p taskcast-cli`
Expected: Compilation errors about missing `tower_http` — that's expected, we fix in Task 2.

---

### Task 2: Replace Filesystem Serving with Embedded Assets

**Files:**
- Modify: `rust/taskcast-cli/src/main.rs`

**Step 1: Ensure the playground dist exists locally for rust-embed**

Run: `cd packages/playground && pnpm build`
Expected: `dist/` directory with `index.html` and `assets/` files.

**Step 2: Add the RustEmbed struct and embedded handler**

Replace lines 104-142 of `main.rs` (the `resolve_playground_dir()` and `nest_playground()` functions) with:

```rust
#[derive(rust_embed::RustEmbed)]
#[folder = "../../packages/playground/dist"]
struct PlaygroundAssets;

/// Serve embedded playground files under /_playground/
fn playground_routes() -> axum::Router {
    axum::Router::new()
        .route("/{*path}", axum::routing::get(serve_playground_asset))
        .route("/", axum::routing::get(serve_playground_index))
}

async fn serve_playground_index() -> axum::response::Response {
    serve_playground_file("index.html")
}

async fn serve_playground_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> axum::response::Response {
    serve_playground_file(&path)
}

fn serve_playground_file(path: &str) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    match PlaygroundAssets::get(path) {
        Some(asset) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                asset.data,
            )
                .into_response()
        }
        None => {
            // SPA fallback: serve index.html for unknown paths
            match PlaygroundAssets::get("index.html") {
                Some(index) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    index.data,
                )
                    .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    }
}
```

**Step 3: Update the `Commands::Start` handler (lines 350-364)**

Replace the playground block:
```rust
// Old:
let app = if playground {
    match resolve_playground_dir() {
        Some(dist_dir) => {
            println!("[taskcast] Playground UI at http://localhost:{port}/_playground/");
            nest_playground(app, &dist_dir)
        }
        None => {
            eprintln!("[taskcast] Playground dist not found. Build the playground first.");
            app
        }
    }
} else {
    app
};

// New:
let app = if playground {
    println!("[taskcast] Playground UI at http://localhost:{port}/_playground/");
    app.nest("/_playground", playground_routes())
} else {
    app
};
```

**Step 4: Update the `Commands::Playground` handler (lines 370-388)**

Replace:
```rust
// Old:
Commands::Playground { port } => {
    match resolve_playground_dir() {
        Some(dist_dir) => {
            let app = axum::Router::new();
            let app = nest_playground(app, &dist_dir);
            let app = app.route("/", axum::routing::get(|| async {
                axum::response::Redirect::temporary("/_playground/")
            }));
            // ...
        }
        None => {
            eprintln!("[taskcast] Playground dist not found. Build the playground first.");
            std::process::exit(1);
        }
    }
}

// New:
Commands::Playground { port } => {
    let app = axum::Router::new()
        .nest("/_playground", playground_routes())
        .route("/", axum::routing::get(|| async {
            axum::response::Redirect::temporary("/_playground/")
        }));
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("[taskcast] Playground UI at http://localhost:{port}/_playground/");
    println!("[taskcast] Use \"External\" mode in the UI to connect to a remote server.");
    axum::serve(listener, app).await?;
}
```

**Step 5: Verify compilation and run tests**

Run: `cd rust && cargo check -p taskcast-cli && cargo test -p taskcast-cli`
Expected: All existing tests pass. No `tower_http` or `resolve_playground_dir` references remain.

**Step 6: Commit**

```bash
git add rust/taskcast-cli/Cargo.toml rust/taskcast-cli/src/main.rs rust/Cargo.lock
git commit -m "feat(rust-cli): embed playground via rust-embed, remove filesystem lookup"
```

---

### Task 3: Add Embedded Asset Tests

**Files:**
- Modify: `rust/taskcast-cli/src/main.rs` (test module)

**Step 1: Write tests for the embedded asset serving**

Add to the `#[cfg(test)] mod tests` block:

```rust
// ─── Embedded playground assets ─────────────────────────────────────

#[test]
fn playground_assets_contains_index_html() {
    assert!(
        PlaygroundAssets::get("index.html").is_some(),
        "index.html must be embedded"
    );
}

#[test]
fn playground_assets_index_html_has_content() {
    let asset = PlaygroundAssets::get("index.html").unwrap();
    let content = std::str::from_utf8(&asset.data).unwrap();
    assert!(content.contains("<!DOCTYPE html>") || content.contains("<html"),
        "index.html should contain HTML");
}

#[test]
fn serve_playground_file_returns_html_for_index() {
    let response = serve_playground_file("index.html");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[test]
fn serve_playground_file_spa_fallback_for_unknown_path() {
    let response = serve_playground_file("nonexistent/route");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[test]
fn serve_playground_file_returns_css_with_correct_mime() {
    // Find a CSS file in the embedded assets
    let css_file = PlaygroundAssets::iter()
        .find(|f| f.ends_with(".css"))
        .expect("should have a CSS file");
    let response = serve_playground_file(&css_file);
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[test]
fn serve_playground_file_returns_js_with_correct_mime() {
    let js_file = PlaygroundAssets::iter()
        .find(|f| f.ends_with(".js"))
        .expect("should have a JS file");
    let response = serve_playground_file(&js_file);
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}
```

**Step 2: Run tests**

Run: `cd rust && cargo test -p taskcast-cli`
Expected: All tests pass (both new and existing).

**Step 3: Commit**

```bash
git add rust/taskcast-cli/src/main.rs
git commit -m "test(rust-cli): add embedded playground asset tests"
```

---

### Task 4: Update rust.yml CI Workflow

**Files:**
- Modify: `.github/workflows/rust.yml`

**Step 1: Add playground build steps to all 4 jobs**

For each of the `check`, `test`, `coverage`, and `build` jobs, add these steps **after** the Rust cache step and **before** any `cargo` commands:

```yaml
      - uses: actions/setup-node@v4
        with:
          node-version: '22'
      - uses: pnpm/action-setup@v4
      - name: Build playground dist
        run: |
          pnpm install --frozen-lockfile
          pnpm --filter @taskcast/playground build
```

Also update the `paths` trigger to include `packages/playground/**`:

```yaml
on:
  push:
    branches: [main, feature/*]
    paths:
      - 'rust/**'
      - 'packages/playground/**'
      - '.github/workflows/rust.yml'
  pull_request:
    branches: [main]
    paths:
      - 'rust/**'
      - 'packages/playground/**'
      - '.github/workflows/rust.yml'
```

**Step 2: Verify YAML is valid**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/rust.yml'))"`
(or just visually inspect indentation)

**Step 3: Commit**

```bash
git add .github/workflows/rust.yml
git commit -m "ci: build playground before Rust compilation in rust.yml"
```

---

### Task 5: Update release.yml CI Workflow

**Files:**
- Modify: `.github/workflows/release.yml`

**Step 1: Add playground build to rust-build job**

In the `rust-build` job, add pnpm setup and playground build **after** Node.js setup (line 184) and **before** "Sync Rust version" (line 186):

```yaml
      - name: Setup pnpm
        uses: pnpm/action-setup@v4

      - name: Build playground dist
        run: |
          pnpm install --frozen-lockfile
          pnpm --filter @taskcast/playground build
```

**Step 2: Add playground build to docker-build job**

In the `docker-build` job, add pnpm and playground build **after** Node.js setup (line 236) and **before** "Sync Rust version" (line 240):

```yaml
      - name: Setup pnpm
        uses: pnpm/action-setup@v4

      - name: Build playground dist
        run: |
          pnpm install --frozen-lockfile
          pnpm --filter @taskcast/playground build
```

**Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: build playground before Rust builds in release.yml"
```

---

### Task 6: Update Dockerfile

**Files:**
- Modify: `rust/Dockerfile`

**Step 1: Add Node.js stage to build playground**

The Dockerfile's build context is `./rust`, but playground source is at `packages/playground/` in the repo root. Two options:
- Change Docker build context to repo root (impacts release.yml)
- Copy pre-built dist into the Rust context before Docker build

The simplest approach: in the `docker-build` CI job, copy the already-built playground dist into `rust/playground-dist/` before Docker runs, then reference that in the Dockerfile.

Update `docker-build` job in `release.yml` — add after the playground build step:
```yaml
      - name: Copy playground dist for Docker
        run: cp -r packages/playground/dist rust/playground-dist
```

Then update `rust/Dockerfile` to:

```dockerfile
# ── Stage 1: chef ─────────────────────────────────────────────
FROM rust:1-slim AS chef
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef
WORKDIR /app

# ── Stage 2: planner ─────────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: builder ─────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# playground-dist is copied into rust/ context by CI before docker build
# rust-embed expects it at ../../packages/playground/dist relative to Cargo.toml
# so we create the expected path
RUN mkdir -p /app/../../packages/playground && \
    cp -r /app/playground-dist /app/../../packages/playground/dist || true
RUN cargo build --release -p taskcast-cli

# ── Stage 4: runtime ─────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/taskcast /usr/local/bin/taskcast

EXPOSE 3721
CMD ["taskcast"]
```

Note: The `../../packages/playground/dist` path is relative to the Cargo.toml at `/app/taskcast-cli/Cargo.toml`. Since Docker context is `./rust` (the workspace root), the builder WORKDIR is `/app`. The rust-embed path `../../packages/playground/dist` resolves from `taskcast-cli/` which is `/app/taskcast-cli/`. So the target is `/packages/playground/dist` (absolute). We need to create that path:

```dockerfile
RUN mkdir -p /packages/playground && cp -r /app/playground-dist /packages/playground/dist || true
```

**Important:** Verify the exact path resolution by checking what `../../packages/playground/dist` resolves to relative to the Cargo.toml location in the Docker build. The Cargo.toml is at `/app/taskcast-cli/Cargo.toml`, so `../../packages/playground/dist` = `/packages/playground/dist`.

**Step 2: Add `playground-dist/` to `rust/.dockerignore` (if exists) or `.gitignore`**

Add to `rust/.gitignore` (or create it):
```
playground-dist/
```

**Step 3: Commit**

```bash
git add rust/Dockerfile rust/.gitignore .github/workflows/release.yml
git commit -m "ci: embed playground dist in Docker builds"
```

---

### Task 7: Final Verification

**Step 1: Run full Rust test suite**

Run: `cd rust && cargo test --workspace`
Expected: All tests pass.

**Step 2: Build release binary and verify playground works**

Run:
```bash
cd rust && cargo build --release -p taskcast-cli
./target/release/taskcast start --playground &
sleep 2
curl -s http://localhost:3721/_playground/ | head -5
kill %1
```
Expected: HTML response containing `<!DOCTYPE html>` or similar.

**Step 3: Verify binary size is reasonable**

Run: `ls -lh rust/target/release/taskcast`
Expected: ~15-16MB (only ~200KB increase from embedded playground).

**Step 4: Create PR**

```bash
git push origin <branch>
gh pr create --title "feat(rust-cli): embed playground in binary via rust-embed" --body "..."
```
