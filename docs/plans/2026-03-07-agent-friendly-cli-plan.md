# Agent-Friendly CLI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add CLI client commands, node management, verbose mode, and skill updates to make Taskcast agent-friendly — in both TypeScript and Rust.

**Architecture:** Client commands use only `@taskcast/server-sdk` (TS) or `reqwest` (Rust) — no server dependencies. Node config stored in `~/.taskcast/nodes.json`. Two new server endpoints (`/health/detail`, `GET /events`) support doctor and tail commands.

**Tech Stack:** TypeScript (Commander.js, Hono, vitest), Rust (clap, axum, reqwest, axum-test)

---

## Task 1: Server — `/health/detail` endpoint (TypeScript)

**Files:**
- Modify: `packages/server/src/index.ts`
- Modify: `packages/server/src/routes/tasks.ts` (reference for pattern)
- Test: `packages/server/tests/health-detail.test.ts`

**Step 1: Write the failing test**

Create `packages/server/tests/health-detail.test.ts`:

```typescript
import { describe, it, expect } from 'vitest'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

function makeApp(opts?: { authMode?: 'none' | 'jwt' }) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const { app } = createTaskcastApp({
    engine,
    auth: { mode: opts?.authMode ?? 'none' },
  })
  return { app, engine }
}

describe('GET /health/detail', () => {
  it('returns version and adapter status', async () => {
    const { app } = makeApp()
    const res = await app.request('/health/detail')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.ok).toBe(true)
    expect(body.auth).toEqual({ mode: 'none' })
    expect(body.adapters.broadcast).toEqual({ provider: 'memory', status: 'ok' })
    expect(body.adapters.shortTermStore).toEqual({ provider: 'memory', status: 'ok' })
  })

  it('includes uptime in seconds', async () => {
    const { app } = makeApp()
    const res = await app.request('/health/detail')
    const body = await res.json()
    expect(typeof body.uptime).toBe('number')
    expect(body.uptime).toBeGreaterThanOrEqual(0)
  })

  it('reports auth mode', async () => {
    const { app } = makeApp({ authMode: 'jwt' })
    const res = await app.request('/health/detail')
    const body = await res.json()
    expect(body.auth.mode).toBe('jwt')
  })
})
```

**Step 2: Run test to verify it fails**

Run: `cd packages/server && pnpm test -- --run tests/health-detail.test.ts`
Expected: FAIL — `/health/detail` returns 404

**Step 3: Implement `/health/detail` endpoint**

In `packages/server/src/index.ts`, after the existing `GET /health` handler (around line 74), add:

```typescript
const startTime = Date.now()

app.get('/health/detail', (c) => {
  const uptime = Math.floor((Date.now() - startTime) / 1000)
  const authMode = opts.auth?.mode ?? 'none'

  // Determine adapter providers from engine/config
  const broadcastProvider = opts.config?.adapters?.broadcast?.provider ?? 'memory'
  const shortTermProvider = opts.config?.adapters?.shortTermStore?.provider ?? 'memory'
  const longTermProvider = opts.config?.adapters?.longTermStore?.provider ?? undefined

  const adapters: Record<string, { provider: string; status: string }> = {
    broadcast: { provider: broadcastProvider, status: 'ok' },
    shortTermStore: { provider: shortTermProvider, status: 'ok' },
  }

  if (longTermProvider) {
    adapters['longTermStore'] = { provider: longTermProvider, status: 'ok' }
  }

  return c.json({ ok: true, uptime, auth: { mode: authMode }, adapters })
})
```

Note: The `startTime` should be captured at the top of `createTaskcastApp`.

**Step 4: Run test to verify it passes**

Run: `cd packages/server && pnpm test -- --run tests/health-detail.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/server/src/index.ts packages/server/tests/health-detail.test.ts
git commit -m "feat(server): add /health/detail endpoint for diagnostics"
```

---

## Task 2: Server — `/health/detail` endpoint (Rust)

**Files:**
- Modify: `rust/taskcast-server/src/app.rs`
- Test: `rust/taskcast-server/tests/health_detail.rs`

**Step 1: Write the failing test**

Create `rust/taskcast-server/tests/health_detail.rs`:

```rust
use axum_test::TestServer;
use std::sync::Arc;
use taskcast_core::{
    memory_adapters::{MemoryBroadcastProvider, MemoryShortTermStore},
    TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode};

fn make_server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(engine, AuthMode::None, None, None);
    TestServer::new(app).unwrap()
}

#[tokio::test]
async fn health_detail_returns_adapter_status() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["auth"]["mode"], "none");
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["broadcast"]["status"], "ok");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["status"], "ok");
}

#[tokio::test]
async fn health_detail_includes_uptime() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    let body: serde_json::Value = res.json();
    assert!(body["uptime"].is_number());
    assert!(body["uptime"].as_f64().unwrap() >= 0.0);
}
```

**Step 2: Run test to verify it fails**

Run: `cd rust && cargo test --package taskcast-server --test health_detail`
Expected: FAIL — compilation error or 404

**Step 3: Implement**

In `rust/taskcast-server/src/app.rs`:

1. Add `startTime: Instant` to `AppState`:
```rust
use std::time::Instant;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<TaskEngine>,
    pub auth_mode: Arc<AuthMode>,
    pub start_time: Instant,
}
```

2. Add handler after the existing `health()` handler:
```rust
async fn health_detail(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    let auth_mode = match state.auth_mode.as_ref() {
        AuthMode::None => "none",
        AuthMode::Jwt(_) => "jwt",
    };

    // TODO: When config is available in state, use actual adapter providers
    axum::Json(serde_json::json!({
        "ok": true,
        "uptime": uptime,
        "auth": { "mode": auth_mode },
        "adapters": {
            "broadcast": { "provider": "memory", "status": "ok" },
            "shortTermStore": { "provider": "memory", "status": "ok" }
        }
    }))
}
```

3. Mount in `create_app()` next to `/health`:
```rust
.route("/health/detail", get(health_detail))
```

4. Initialize `start_time: Instant::now()` in the AppState construction.

**Step 4: Run test to verify it passes**

Run: `cd rust && cargo test --package taskcast-server --test health_detail`
Expected: PASS

**Step 5: Commit**

```bash
git add rust/taskcast-server/src/app.rs rust/taskcast-server/tests/health_detail.rs
git commit -m "feat(server-rs): add /health/detail endpoint for diagnostics"
```

---

## Task 3: Server — Global SSE endpoint `GET /events` (TypeScript)

**Files:**
- Modify: `packages/server/src/index.ts`
- Modify: `packages/server/src/routes/sse.ts`
- Test: `packages/server/tests/global-sse.test.ts`

**Step 1: Write the failing test**

Create `packages/server/tests/global-sse.test.ts`:

```typescript
import { describe, it, expect } from 'vitest'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })
  return { app, engine }
}

describe('GET /events (global SSE)', () => {
  it('streams events from any task', async () => {
    const { app, engine } = makeApp()

    // Create two tasks and publish events
    const task1 = await engine.createTask({ type: 'llm.chat' })
    const task2 = await engine.createTask({ type: 'agent.step' })
    await engine.transitionTask(task1.id, 'running')
    await engine.transitionTask(task2.id, 'running')
    await engine.publishEvent(task1.id, { type: 'llm.delta', level: 'info', data: { delta: 'hello' } })
    await engine.publishEvent(task2.id, { type: 'agent.log', level: 'info', data: { step: 1 } })

    // Complete both so SSE closes
    await engine.transitionTask(task1.id, 'completed')
    await engine.transitionTask(task2.id, 'completed')

    const res = await app.request('/events')
    expect(res.status).toBe(200)
    expect(res.headers.get('content-type')).toContain('text/event-stream')
  })

  it('filters by event type', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'llm.chat' })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { delta: 'hi' } })
    await engine.publishEvent(task.id, { type: 'agent.log', level: 'info', data: { x: 1 } })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request('/events?types=llm.*')
    expect(res.status).toBe(200)
  })
})
```

**Step 2: Run test to verify it fails**

Run: `cd packages/server && pnpm test -- --run tests/global-sse.test.ts`
Expected: FAIL — `/events` returns 404

**Step 3: Implement global SSE**

This endpoint needs to:
1. Listen to the engine's creation listener to discover new tasks
2. Subscribe to each task's broadcast channel
3. Aggregate all events into one SSE stream
4. Apply filter (types, levels) across all events
5. Include `taskId` in each envelope

Add a new route in `packages/server/src/routes/sse.ts` — a `createGlobalSSERouter(engine)` function — and mount it in `index.ts` at `GET /events`.

The implementation subscribes to the engine's broadcast for ALL tasks by using `engine.addCreationListener` and `engine.subscribe`. On new task creation, subscribe to that task's events and forward them.

For the initial version, this streams events from tasks created AFTER the SSE connection is established. Historical replay across all tasks is not needed.

**Step 4: Run test to verify it passes**

Run: `cd packages/server && pnpm test -- --run tests/global-sse.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/server/src/routes/sse.ts packages/server/src/index.ts packages/server/tests/global-sse.test.ts
git commit -m "feat(server): add global SSE endpoint GET /events"
```

---

## Task 4: Server — Global SSE endpoint `GET /events` (Rust)

**Files:**
- Modify: `rust/taskcast-server/src/app.rs`
- Modify: `rust/taskcast-server/src/routes/sse.rs`
- Test: `rust/taskcast-server/tests/global_sse.rs`

**Step 1: Write the failing test**

Create `rust/taskcast-server/tests/global_sse.rs` with similar tests to Task 3 but using `axum_test::TestServer`. Test that `GET /events` returns SSE content-type and streams events.

**Step 2: Run test to verify it fails**

Run: `cd rust && cargo test --package taskcast-server --test global_sse`

**Step 3: Implement**

Add `global_sse_events()` handler in `routes/sse.rs`. Use a similar pattern to the per-task SSE handler but subscribe to all tasks via `engine.add_creation_listener()`.

Mount as `GET /events` in `app.rs`.

**Step 4: Run test, verify pass**

**Step 5: Commit**

```bash
git add rust/taskcast-server/src/routes/sse.rs rust/taskcast-server/src/app.rs rust/taskcast-server/tests/global_sse.rs
git commit -m "feat(server-rs): add global SSE endpoint GET /events"
```

---

## Task 5: CLI — Refactor existing commands into separate files (TypeScript)

**Files:**
- Create: `packages/cli/src/commands/start.ts`
- Create: `packages/cli/src/commands/migrate.ts`
- Create: `packages/cli/src/commands/playground.ts`
- Create: `packages/cli/src/commands/ui.ts`
- Modify: `packages/cli/src/index.ts`

**Step 1: Create `packages/cli/src/commands/` directory**

Run: `mkdir -p packages/cli/src/commands`

**Step 2: Extract `start` command**

Move the `start` command handler (lines 100-220 of current `index.ts`) into `commands/start.ts`:

```typescript
import { Command } from 'commander'
// ... imports for Redis, postgres, engine, etc.

export function registerStartCommand(program: Command): void {
  program
    .command('start', { isDefault: true })
    .description('Start the taskcast server in foreground (default)')
    .option('-c, --config <path>', 'config file path')
    .option('-p, --port <port>', 'port to listen on', '3721')
    .option('-s, --storage <type>', 'storage backend: memory | redis | sqlite', 'memory')
    .option('--db-path <path>', 'SQLite database file path (default: ./taskcast.db)')
    .option('--playground', 'serve the interactive playground UI at /_playground/')
    .option('-v, --verbose', 'enable verbose request logging')
    .action(async (options) => {
      // ... existing start logic moved here
    })
}
```

**Step 3: Extract `migrate`, `playground`, `ui` commands similarly**

Each gets its own file with a `registerXxxCommand(program: Command)` export.

**Step 4: Slim down `index.ts`**

```typescript
#!/usr/bin/env node
import { Command } from 'commander'
import { registerStartCommand } from './commands/start.js'
import { registerMigrateCommand } from './commands/migrate.js'
import { registerPlaygroundCommand } from './commands/playground.js'
import { registerUiCommand } from './commands/ui.js'

const program = new Command()
program
  .name('taskcast')
  .description('Taskcast — unified task tracking and streaming service')
  .version('0.3.1')

registerStartCommand(program)
registerMigrateCommand(program)
registerPlaygroundCommand(program)
registerUiCommand(program)

// Placeholders for new commands (will be added in later tasks)
program.command('daemon').description('(not yet implemented)').action(() => {
  console.error('[taskcast] daemon mode is not yet implemented')
  process.exit(1)
})
program.command('stop').description('(not yet implemented)').action(() => {
  console.error('[taskcast] stop is not yet implemented')
  process.exit(1)
})
program.command('status').description('(not yet implemented)').action(() => {
  console.error('[taskcast] status is not yet implemented')
  process.exit(1)
})

program.parse()
```

**Step 5: Run existing tests to verify nothing broke**

Run: `cd packages/cli && pnpm test`
Expected: All existing tests PASS

**Step 6: Commit**

```bash
git add packages/cli/src/
git commit -m "refactor(cli): split commands into separate files"
```

---

## Task 6: CLI — Refactor existing commands into separate files (Rust)

**Files:**
- Create: `rust/taskcast-cli/src/commands/mod.rs`
- Create: `rust/taskcast-cli/src/commands/start.rs`
- Create: `rust/taskcast-cli/src/commands/migrate.rs`
- Create: `rust/taskcast-cli/src/commands/playground.rs`
- Modify: `rust/taskcast-cli/src/main.rs`

**Step 1: Create `commands/` module directory**

Run: `mkdir -p rust/taskcast-cli/src/commands`

**Step 2: Extract start command logic**

Move the `Commands::Start` handler from `main.rs` (lines ~202-407) into `commands/start.rs`:

```rust
use std::sync::Arc;
use clap::Args;
// ... other imports

#[derive(Args)]
pub struct StartArgs {
    #[arg(short, long)]
    pub config: Option<String>,
    #[arg(short, long, default_value = "3721")]
    pub port: u16,
    #[arg(short, long, default_value = "memory")]
    pub storage: String,
    #[arg(long, default_value = "./taskcast.db")]
    pub db_path: String,
    #[arg(long)]
    pub playground: bool,
    #[arg(short, long)]
    pub verbose: bool,
}

pub async fn run(args: StartArgs) -> anyhow::Result<()> {
    // ... existing start logic
}
```

**Step 3: Extract `migrate`, `playground` similarly**

**Step 4: Update `main.rs` to use modules**

```rust
mod commands;

#[derive(Parser)]
#[command(name = "taskcast", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Start(commands::start::StartArgs),
    Migrate(commands::migrate::MigrateArgs),
    Playground(commands::playground::PlaygroundArgs),
    // ... new commands will be added here later
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Start(Default::default())) {
        Commands::Start(args) => commands::start::run(args).await,
        Commands::Migrate(args) => commands::migrate::run(args).await,
        Commands::Playground(args) => commands::playground::run(args).await,
    }
}
```

**Step 5: Run existing tests**

Run: `cd rust && cargo test --package taskcast-cli`
Expected: All existing tests PASS

**Step 6: Commit**

```bash
git add rust/taskcast-cli/src/
git commit -m "refactor(cli-rs): split commands into separate files"
```

---

## Task 7: CLI — Node config module (TypeScript)

**Files:**
- Create: `packages/cli/src/node-config.ts`
- Test: `packages/cli/tests/unit/node-config.test.ts`

**Step 1: Write the failing tests**

Create `packages/cli/tests/unit/node-config.test.ts`:

```typescript
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mkdtempSync, rmSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import { NodeConfigManager } from '../../src/node-config.js'

describe('NodeConfigManager', () => {
  let dir: string
  let mgr: NodeConfigManager

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-test-'))
    mgr = new NodeConfigManager(dir)
  })

  afterEach(() => {
    rmSync(dir, { recursive: true, force: true })
  })

  it('returns default localhost when no nodes configured', () => {
    const node = mgr.getCurrent()
    expect(node).toEqual({ url: 'http://localhost:3721' })
  })

  it('adds and retrieves a node', () => {
    mgr.add('prod', { url: 'https://tc.example.com', token: 'ey123', tokenType: 'jwt' })
    const node = mgr.get('prod')
    expect(node).toEqual({ url: 'https://tc.example.com', token: 'ey123', tokenType: 'jwt' })
  })

  it('sets and gets current node', () => {
    mgr.add('prod', { url: 'https://tc.example.com' })
    mgr.use('prod')
    const current = mgr.getCurrent()
    expect(current.url).toBe('https://tc.example.com')
  })

  it('removes a node', () => {
    mgr.add('staging', { url: 'https://staging.tc.io' })
    mgr.remove('staging')
    expect(mgr.get('staging')).toBeUndefined()
  })

  it('lists all nodes', () => {
    mgr.add('a', { url: 'http://a' })
    mgr.add('b', { url: 'http://b' })
    const list = mgr.list()
    expect(list).toHaveLength(2)
    expect(list.map(n => n.name)).toContain('a')
    expect(list.map(n => n.name)).toContain('b')
  })

  it('marks current node in list', () => {
    mgr.add('a', { url: 'http://a' })
    mgr.use('a')
    const list = mgr.list()
    expect(list.find(n => n.name === 'a')?.current).toBe(true)
  })

  it('throws when using a non-existent node', () => {
    expect(() => mgr.use('nope')).toThrow('Node "nope" not found')
  })

  it('throws when removing a non-existent node', () => {
    expect(() => mgr.remove('nope')).toThrow('Node "nope" not found')
  })

  it('resets current to default when current node is removed', () => {
    mgr.add('prod', { url: 'https://tc.example.com' })
    mgr.use('prod')
    mgr.remove('prod')
    const current = mgr.getCurrent()
    expect(current.url).toBe('http://localhost:3721')
  })

  it('persists across instances', () => {
    mgr.add('prod', { url: 'https://tc.example.com', token: 'tok', tokenType: 'admin' })
    mgr.use('prod')
    const mgr2 = new NodeConfigManager(dir)
    const current = mgr2.getCurrent()
    expect(current.url).toBe('https://tc.example.com')
    expect(current.tokenType).toBe('admin')
  })
})
```

**Step 2: Run test to verify it fails**

Run: `cd packages/cli && pnpm test -- --run tests/unit/node-config.test.ts`
Expected: FAIL — module not found

**Step 3: Implement `node-config.ts`**

Create `packages/cli/src/node-config.ts`:

```typescript
import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'fs'
import { join } from 'path'
import { homedir } from 'os'

export interface NodeEntry {
  url: string
  token?: string
  tokenType?: 'jwt' | 'admin'
}

interface NodeConfigData {
  current?: string
  nodes: Record<string, NodeEntry>
}

export interface NodeListEntry extends NodeEntry {
  name: string
  current: boolean
}

const DEFAULT_URL = 'http://localhost:3721'

export class NodeConfigManager {
  private filePath: string

  constructor(configDir?: string) {
    const dir = configDir ?? join(homedir(), '.taskcast')
    this.filePath = join(dir, 'nodes.json')
  }

  private load(): NodeConfigData {
    if (!existsSync(this.filePath)) {
      return { nodes: {} }
    }
    return JSON.parse(readFileSync(this.filePath, 'utf-8'))
  }

  private save(data: NodeConfigData): void {
    const dir = this.filePath.replace(/\/[^/]+$/, '')
    mkdirSync(dir, { recursive: true })
    writeFileSync(this.filePath, JSON.stringify(data, null, 2) + '\n')
  }

  getCurrent(): NodeEntry {
    const data = this.load()
    if (data.current && data.nodes[data.current]) {
      return data.nodes[data.current]
    }
    return { url: DEFAULT_URL }
  }

  get(name: string): NodeEntry | undefined {
    return this.load().nodes[name]
  }

  add(name: string, entry: NodeEntry): void {
    const data = this.load()
    data.nodes[name] = entry
    this.save(data)
  }

  remove(name: string): void {
    const data = this.load()
    if (!data.nodes[name]) throw new Error(`Node "${name}" not found`)
    delete data.nodes[name]
    if (data.current === name) data.current = undefined
    this.save(data)
  }

  use(name: string): void {
    const data = this.load()
    if (!data.nodes[name]) throw new Error(`Node "${name}" not found`)
    data.current = name
    this.save(data)
  }

  list(): NodeListEntry[] {
    const data = this.load()
    return Object.entries(data.nodes).map(([name, entry]) => ({
      ...entry,
      name,
      current: data.current === name,
    }))
  }
}
```

**Step 4: Run test to verify it passes**

Run: `cd packages/cli && pnpm test -- --run tests/unit/node-config.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/cli/src/node-config.ts packages/cli/tests/unit/node-config.test.ts
git commit -m "feat(cli): add node config manager for multi-node management"
```

---

## Task 8: CLI — Node config module (Rust)

**Files:**
- Create: `rust/taskcast-cli/src/node_config.rs`
- Test: inline `#[cfg(test)]` module in same file

**Step 1: Write the failing tests**

Add to `rust/taskcast-cli/src/node_config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn returns_default_when_no_config() {
        let dir = TempDir::new().unwrap();
        let mgr = NodeConfigManager::new(dir.path().to_path_buf());
        let current = mgr.get_current();
        assert_eq!(current.url, "http://localhost:3721");
        assert!(current.token.is_none());
    }

    #[test]
    fn add_and_get_node() {
        let dir = TempDir::new().unwrap();
        let mgr = NodeConfigManager::new(dir.path().to_path_buf());
        mgr.add("prod", NodeEntry {
            url: "https://tc.example.com".to_string(),
            token: Some("ey123".to_string()),
            token_type: Some(TokenType::Jwt),
        });
        let node = mgr.get("prod").unwrap();
        assert_eq!(node.url, "https://tc.example.com");
        assert_eq!(node.token_type, Some(TokenType::Jwt));
    }

    #[test]
    fn use_and_get_current() {
        let dir = TempDir::new().unwrap();
        let mgr = NodeConfigManager::new(dir.path().to_path_buf());
        mgr.add("prod", NodeEntry { url: "https://tc.example.com".to_string(), token: None, token_type: None });
        mgr.set_current("prod").unwrap();
        assert_eq!(mgr.get_current().url, "https://tc.example.com");
    }

    #[test]
    fn remove_node() {
        let dir = TempDir::new().unwrap();
        let mgr = NodeConfigManager::new(dir.path().to_path_buf());
        mgr.add("staging", NodeEntry { url: "https://s.io".to_string(), token: None, token_type: None });
        mgr.remove("staging").unwrap();
        assert!(mgr.get("staging").is_none());
    }

    #[test]
    fn remove_current_resets_to_default() {
        let dir = TempDir::new().unwrap();
        let mgr = NodeConfigManager::new(dir.path().to_path_buf());
        mgr.add("prod", NodeEntry { url: "https://tc.example.com".to_string(), token: None, token_type: None });
        mgr.set_current("prod").unwrap();
        mgr.remove("prod").unwrap();
        assert_eq!(mgr.get_current().url, "http://localhost:3721");
    }

    #[test]
    fn persists_across_instances() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        {
            let mgr = NodeConfigManager::new(path.clone());
            mgr.add("prod", NodeEntry { url: "https://tc.example.com".to_string(), token: Some("tok".to_string()), token_type: Some(TokenType::Admin) });
            mgr.set_current("prod").unwrap();
        }
        let mgr2 = NodeConfigManager::new(path);
        let current = mgr2.get_current();
        assert_eq!(current.url, "https://tc.example.com");
        assert_eq!(current.token_type, Some(TokenType::Admin));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd rust && cargo test --package taskcast-cli node_config`
Expected: FAIL — struct not found

**Step 3: Implement `node_config.rs`**

Implement `NodeConfigManager`, `NodeEntry`, `TokenType`, `NodeConfigData` structs with `serde` for JSON serialization. Same logic as the TypeScript version — read/write `nodes.json` in the config dir.

**Step 4: Run test to verify it passes**

Run: `cd rust && cargo test --package taskcast-cli node_config`
Expected: PASS

**Step 5: Commit**

```bash
git add rust/taskcast-cli/src/node_config.rs rust/taskcast-cli/src/main.rs
git commit -m "feat(cli-rs): add node config manager for multi-node management"
```

---

## Task 9: CLI — HTTP client helper (TypeScript)

**Files:**
- Create: `packages/cli/src/client.ts`
- Test: `packages/cli/tests/unit/client.test.ts`

**Step 1: Write the failing tests**

```typescript
import { describe, it, expect, vi } from 'vitest'
import { createClientFromNode } from '../../src/client.js'

describe('createClientFromNode', () => {
  it('creates client with JWT token directly', () => {
    const client = createClientFromNode({ url: 'http://localhost:3721', token: 'jwt-tok', tokenType: 'jwt' })
    expect(client.baseUrl).toBe('http://localhost:3721')
  })

  it('creates client with no token when none provided', () => {
    const client = createClientFromNode({ url: 'http://localhost:3721' })
    expect(client.baseUrl).toBe('http://localhost:3721')
  })

  it('exchanges admin token for JWT', async () => {
    // Mock fetch for admin token exchange
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ token: 'exchanged-jwt', expiresAt: Date.now() + 86400000 }),
    })

    const client = await createClientFromNodeAsync(
      { url: 'http://localhost:3721', token: 'admin_xxx', tokenType: 'admin' },
      mockFetch,
    )
    expect(mockFetch).toHaveBeenCalledWith('http://localhost:3721/admin/token', expect.anything())
    expect(client.baseUrl).toBe('http://localhost:3721')
  })
})
```

**Step 2: Run test, verify fail**

**Step 3: Implement `client.ts`**

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'
import type { NodeEntry } from './node-config.js'

export function createClientFromNode(
  node: NodeEntry,
  fetchFn?: typeof globalThis.fetch,
): TaskcastServerClient {
  return new TaskcastServerClient({
    baseUrl: node.url,
    token: node.tokenType === 'jwt' ? node.token : undefined,
    fetch: fetchFn,
  })
}

export async function createClientFromNodeAsync(
  node: NodeEntry,
  fetchFn: typeof globalThis.fetch = globalThis.fetch,
): Promise<TaskcastServerClient> {
  let token = node.token
  if (node.tokenType === 'admin' && node.token) {
    const res = await fetchFn(`${node.url}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: node.token }),
    })
    if (res.ok) {
      const body = await res.json()
      token = body.token
    } else {
      throw new Error(`Admin token exchange failed: ${res.status}`)
    }
  }

  return new TaskcastServerClient({
    baseUrl: node.url,
    token: node.tokenType !== undefined ? token : undefined,
    fetch: fetchFn,
  })
}
```

**Step 4: Run test, verify pass**

**Step 5: Commit**

```bash
git add packages/cli/src/client.ts packages/cli/tests/unit/client.test.ts
git commit -m "feat(cli): add HTTP client helper with admin token exchange"
```

---

## Task 10: CLI — HTTP client helper (Rust)

**Files:**
- Create: `rust/taskcast-cli/src/client.rs`
- Test: inline `#[cfg(test)]` module

Similar to Task 9 but using `reqwest::Client`. The client struct wraps a `reqwest::Client` with base URL and token, handling admin token exchange via `POST /admin/token`.

**Step 1–5:** Same TDD pattern: write test → verify fail → implement → verify pass → commit.

```bash
git commit -m "feat(cli-rs): add HTTP client helper with admin token exchange"
```

---

## Task 11: CLI — `taskcast node` commands (TypeScript)

**Files:**
- Create: `packages/cli/src/commands/node.ts`
- Modify: `packages/cli/src/index.ts` (register command)
- Test: `packages/cli/tests/unit/node-command.test.ts`

**Step 1: Write the failing tests**

Test the command registration and output formatting — call the command functions directly (not via process spawn) to test output logic.

```typescript
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mkdtempSync, rmSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import { NodeConfigManager } from '../../src/node-config.js'
import { formatNodeList } from '../../src/commands/node.js'

describe('node commands', () => {
  let dir: string

  beforeEach(() => { dir = mkdtempSync(join(tmpdir(), 'tc-test-')) })
  afterEach(() => { rmSync(dir, { recursive: true, force: true }) })

  it('formatNodeList shows current marker', () => {
    const mgr = new NodeConfigManager(dir)
    mgr.add('prod', { url: 'https://tc.example.com', tokenType: 'jwt' })
    mgr.add('local', { url: 'http://localhost:3721' })
    mgr.use('prod')
    const output = formatNodeList(mgr.list())
    expect(output).toContain('* prod')
    expect(output).toContain('  local')
    expect(output).toContain('https://tc.example.com')
  })

  it('formatNodeList shows empty message when no nodes', () => {
    const mgr = new NodeConfigManager(dir)
    const output = formatNodeList(mgr.list())
    expect(output).toContain('No nodes configured')
  })
})
```

**Step 2: Run test, verify fail**

**Step 3: Implement `commands/node.ts`**

```typescript
import { Command } from 'commander'
import { NodeConfigManager } from '../node-config.js'
import type { NodeListEntry } from '../node-config.js'

export function formatNodeList(nodes: NodeListEntry[]): string {
  if (nodes.length === 0) return 'No nodes configured. Using default: http://localhost:3721'
  return nodes.map(n => {
    const marker = n.current ? '* ' : '  '
    const auth = n.tokenType ? ` (${n.tokenType})` : ''
    return `${marker}${n.name}  ${n.url}${auth}`
  }).join('\n')
}

export function registerNodeCommand(program: Command): void {
  const node = program.command('node').description('Manage Taskcast server connections')

  node.command('add <name>')
    .description('Add a node')
    .requiredOption('--url <url>', 'Server URL')
    .option('--token <token>', 'Auth token')
    .option('--token-type <type>', 'Token type: jwt or admin', 'jwt')
    .action((name, opts) => {
      const mgr = new NodeConfigManager()
      mgr.add(name, { url: opts.url, token: opts.token, tokenType: opts.tokenType })
      console.log(`[taskcast] Added node "${name}" → ${opts.url}`)
    })

  node.command('remove <name>')
    .description('Remove a node')
    .action((name) => {
      const mgr = new NodeConfigManager()
      mgr.remove(name)
      console.log(`[taskcast] Removed node "${name}"`)
    })

  node.command('use <name>')
    .description('Set the default node')
    .action((name) => {
      const mgr = new NodeConfigManager()
      mgr.use(name)
      console.log(`[taskcast] Now using node "${name}"`)
    })

  node.command('list')
    .description('List all nodes')
    .action(() => {
      const mgr = new NodeConfigManager()
      console.log(formatNodeList(mgr.list()))
    })
}
```

**Step 4: Register in `index.ts`**

Add `import { registerNodeCommand } from './commands/node.js'` and `registerNodeCommand(program)`.

**Step 5: Run test, verify pass**

**Step 6: Commit**

```bash
git add packages/cli/src/commands/node.ts packages/cli/src/index.ts packages/cli/tests/unit/node-command.test.ts
git commit -m "feat(cli): add taskcast node add/remove/use/list commands"
```

---

## Task 12: CLI — `taskcast node` commands (Rust)

**Files:**
- Create: `rust/taskcast-cli/src/commands/node.rs`
- Modify: `rust/taskcast-cli/src/main.rs` (register subcommand)

Same pattern as Task 11 — clap subcommands with `add`, `remove`, `use`, `list` actions. Uses `node_config.rs` from Task 8.

**Step 1–5:** TDD pattern → commit.

```bash
git commit -m "feat(cli-rs): add taskcast node add/remove/use/list commands"
```

---

## Task 13: CLI — `taskcast ping` command (TypeScript)

**Files:**
- Create: `packages/cli/src/commands/ping.ts`
- Modify: `packages/cli/src/index.ts`
- Test: `packages/cli/tests/unit/ping.test.ts`

**Step 1: Write the failing test**

```typescript
import { describe, it, expect, vi } from 'vitest'
import { pingServer } from '../../src/commands/ping.js'

describe('pingServer', () => {
  it('returns OK with latency on success', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ ok: true }),
    })
    const result = await pingServer('http://localhost:3721', mockFetch)
    expect(result.ok).toBe(true)
    expect(typeof result.latencyMs).toBe('number')
  })

  it('returns FAIL on connection error', async () => {
    const mockFetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED'))
    const result = await pingServer('http://localhost:3721', mockFetch)
    expect(result.ok).toBe(false)
    expect(result.error).toContain('ECONNREFUSED')
  })

  it('returns FAIL on non-200 response', async () => {
    const mockFetch = vi.fn().mockResolvedValue({ ok: false, status: 503 })
    const result = await pingServer('http://localhost:3721', mockFetch)
    expect(result.ok).toBe(false)
  })
})
```

**Step 2: Run test, verify fail**

**Step 3: Implement `commands/ping.ts`**

```typescript
import { Command } from 'commander'
import { NodeConfigManager } from '../node-config.js'

export interface PingResult {
  ok: boolean
  latencyMs?: number
  error?: string
}

export async function pingServer(
  url: string,
  fetchFn: typeof globalThis.fetch = globalThis.fetch,
): Promise<PingResult> {
  const start = Date.now()
  try {
    const res = await fetchFn(`${url}/health`)
    const latencyMs = Date.now() - start
    if (!res.ok) return { ok: false, error: `HTTP ${res.status}` }
    return { ok: true, latencyMs }
  } catch (err) {
    return { ok: false, error: (err as Error).message }
  }
}

export function registerPingCommand(program: Command): void {
  program
    .command('ping')
    .description('Check connectivity to a Taskcast server')
    .option('--node <name>', 'Target node (default: current)')
    .action(async (opts) => {
      const mgr = new NodeConfigManager()
      const node = opts.node ? mgr.get(opts.node) : mgr.getCurrent()
      if (!node) { console.error(`[taskcast] Node "${opts.node}" not found`); process.exit(1) }
      const result = await pingServer(node.url)
      if (result.ok) {
        console.log(`OK — taskcast at ${node.url} (${result.latencyMs}ms)`)
      } else {
        console.error(`FAIL — cannot reach ${node.url}: ${result.error}`)
        process.exit(1)
      }
    })
}
```

**Step 4: Register in `index.ts`, run tests, verify pass**

**Step 5: Commit**

```bash
git add packages/cli/src/commands/ping.ts packages/cli/src/index.ts packages/cli/tests/unit/ping.test.ts
git commit -m "feat(cli): add taskcast ping command"
```

---

## Task 14: CLI — `taskcast ping` command (Rust)

Same pattern. Uses `reqwest::get()` to hit `/health`. Measures latency with `Instant::now()`.

```bash
git commit -m "feat(cli-rs): add taskcast ping command"
```

---

## Task 15: CLI — `taskcast doctor` command (TypeScript)

**Files:**
- Create: `packages/cli/src/commands/doctor.ts`
- Modify: `packages/cli/src/index.ts`
- Test: `packages/cli/tests/unit/doctor.test.ts`

**Step 1: Write the failing test**

```typescript
import { describe, it, expect, vi } from 'vitest'
import { runDoctor } from '../../src/commands/doctor.js'

describe('runDoctor', () => {
  it('reports all OK when server healthy', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        ok: true,
        uptime: 120,
        auth: { mode: 'none' },
        adapters: {
          broadcast: { provider: 'memory', status: 'ok' },
          shortTermStore: { provider: 'memory', status: 'ok' },
        },
      }),
    })
    const result = await runDoctor({ url: 'http://localhost:3721' }, mockFetch)
    expect(result.server.ok).toBe(true)
    expect(result.adapters.broadcast.status).toBe('ok')
  })

  it('reports FAIL when server unreachable', async () => {
    const mockFetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED'))
    const result = await runDoctor({ url: 'http://localhost:3721' }, mockFetch)
    expect(result.server.ok).toBe(false)
  })
})
```

**Step 2–5:** Implement `runDoctor()` that calls `GET /health/detail`, parses response, returns structured result. `registerDoctorCommand` formats and prints it.

```bash
git commit -m "feat(cli): add taskcast doctor command"
```

---

## Task 16: CLI — `taskcast doctor` command (Rust)

Same pattern — `reqwest::get("/health/detail")`, parse JSON, format output.

```bash
git commit -m "feat(cli-rs): add taskcast doctor command"
```

---

## Task 17: CLI — `taskcast tasks list` and `taskcast tasks inspect` (TypeScript)

**Files:**
- Create: `packages/cli/src/commands/tasks.ts`
- Modify: `packages/cli/src/index.ts`
- Test: `packages/cli/tests/unit/tasks-command.test.ts`
- Test: `packages/cli/tests/integration/tasks-command.test.ts`

**Step 1: Write unit tests for output formatting**

```typescript
import { describe, it, expect } from 'vitest'
import { formatTaskList, formatTaskInspect } from '../../src/commands/tasks.js'

describe('formatTaskList', () => {
  it('formats tasks as table', () => {
    const tasks = [
      { id: '01JXXXXX', type: 'llm.chat', status: 'running', createdAt: 1709827801000 },
      { id: '01JYYYYY', type: 'agent.step', status: 'completed', createdAt: 1709827802000 },
    ]
    const output = formatTaskList(tasks)
    expect(output).toContain('01JXXXXX')
    expect(output).toContain('llm.chat')
    expect(output).toContain('running')
  })

  it('shows empty message when no tasks', () => {
    expect(formatTaskList([])).toContain('No tasks found')
  })
})
```

**Step 2: Write integration test (against real in-memory server)**

```typescript
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

describe('tasks commands integration', () => {
  let app: any
  let engine: TaskEngine
  let baseUrl: string

  beforeAll(async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    engine = new TaskEngine({ shortTermStore: store, broadcast })
    const result = createTaskcastApp({ engine, auth: { mode: 'none' } })
    app = result.app
    // Use app.request() for testing
  })

  it('lists tasks via HTTP', async () => {
    await engine.createTask({ type: 'llm.chat' })
    const res = await app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks.length).toBeGreaterThan(0)
  })
})
```

**Step 3: Implement `commands/tasks.ts`**

Uses `createClientFromNodeAsync()` to get a client, then:
- `list`: GET `/tasks` with query params for status/type/limit
- `inspect`: GET `/tasks/{taskId}` + GET `/tasks/{taskId}/events/history` (last 10 events)

**Step 4: Register, run tests, verify pass**

**Step 5: Commit**

```bash
git commit -m "feat(cli): add taskcast tasks list/inspect commands"
```

---

## Task 18: CLI — `taskcast tasks list/inspect` (Rust)

Same feature in Rust using `reqwest`.

```bash
git commit -m "feat(cli-rs): add taskcast tasks list/inspect commands"
```

---

## Task 19: CLI — `taskcast logs` command (TypeScript)

**Files:**
- Create: `packages/cli/src/commands/logs.ts`
- Modify: `packages/cli/src/index.ts`
- Test: `packages/cli/tests/unit/logs.test.ts`
- Test: `packages/cli/tests/integration/logs.test.ts`

**Step 1: Write unit tests**

Test the event formatting function:

```typescript
import { describe, it, expect } from 'vitest'
import { formatEvent } from '../../src/commands/logs.js'

describe('formatEvent', () => {
  it('formats event as log line', () => {
    const event = {
      type: 'llm.delta',
      level: 'info',
      timestamp: 1709827801000,
      data: { delta: 'hello' },
    }
    const line = formatEvent(event)
    expect(line).toContain('llm.delta')
    expect(line).toContain('info')
    expect(line).toContain('"delta":"hello"')
  })

  it('formats done event', () => {
    const line = formatEvent({ type: 'taskcast:done', level: 'info', timestamp: Date.now(), data: { reason: 'completed' } })
    expect(line).toContain('[DONE]')
    expect(line).toContain('completed')
  })
})
```

**Step 2: Write integration test**

Test against in-memory server — create a task, publish events, complete it, then read the SSE stream and verify all events arrive.

**Step 3: Implement `commands/logs.ts`**

The command:
1. Resolves the current node
2. Opens an SSE connection to `GET /tasks/{taskId}/events` with optional `?types=...&levels=...`
3. Parses each SSE event and prints a formatted log line
4. Exits when `taskcast.done` event received

Uses native `fetch()` with streaming response body or the `eventsource` npm package for SSE parsing.

**Step 4: Register, run tests, verify pass**

**Step 5: Commit**

```bash
git commit -m "feat(cli): add taskcast logs command for real-time event streaming"
```

---

## Task 20: CLI — `taskcast logs` command (Rust)

Same feature using `reqwest` with streaming response. Parse SSE lines manually (`data:` / `event:` / `id:` format).

```bash
git commit -m "feat(cli-rs): add taskcast logs command for real-time event streaming"
```

---

## Task 21: CLI — `taskcast tail` command (TypeScript)

**Files:**
- Create: `packages/cli/src/commands/tail.ts` (or extend `logs.ts`)
- Modify: `packages/cli/src/index.ts`
- Test: `packages/cli/tests/integration/tail.test.ts`

**Step 1: Write integration test**

Test against in-memory server with global SSE endpoint (from Task 3):
- Connect to `GET /events`
- Create tasks and publish events
- Verify events from multiple tasks appear in the stream

**Step 2: Implement**

Similar to `logs` but connects to `GET /events` (global endpoint). Each log line includes truncated task ID prefix.

Format:
```
[14:30:02] 01JXX..  llm.delta    info  {"delta": "Hello "}
```

**Step 3: Register, run tests, verify pass**

**Step 4: Commit**

```bash
git commit -m "feat(cli): add taskcast tail command for global event stream"
```

---

## Task 22: CLI — `taskcast tail` command (Rust)

```bash
git commit -m "feat(cli-rs): add taskcast tail command for global event stream"
```

---

## Task 23: CLI — `--verbose` mode for `taskcast start` (TypeScript)

**Files:**
- Modify: `packages/cli/src/commands/start.ts`
- Create: `packages/server/src/middleware/verbose-logger.ts`
- Test: `packages/server/tests/verbose-logger.test.ts`

**Step 1: Write the failing test**

```typescript
import { describe, it, expect } from 'vitest'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

describe('verbose logger middleware', () => {
  it('logs requests when verbose enabled', async () => {
    const logs: string[] = []
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const { app } = createTaskcastApp({
      engine,
      auth: { mode: 'none' },
      verbose: true,
      verboseLogger: (line: string) => logs.push(line),
    })

    await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.chat' }),
    })

    expect(logs.length).toBeGreaterThan(0)
    expect(logs[0]).toContain('POST')
    expect(logs[0]).toContain('/tasks')
    expect(logs[0]).toContain('201')
  })
})
```

**Step 2: Run test, verify fail**

**Step 3: Implement verbose logger**

Create a Hono middleware that logs method, path, status code, duration, and context-specific info (task ID from response body, transition info from request body).

Add `verbose?: boolean` and `verboseLogger?: (line: string) => void` options to `createTaskcastApp`.

**Step 4: Run test, verify pass**

**Step 5: Wire up in `commands/start.ts` — when `--verbose` flag is set, pass `verbose: true` to `createTaskcastApp`**

**Step 6: Commit**

```bash
git add packages/server/src/middleware/verbose-logger.ts packages/server/tests/verbose-logger.test.ts packages/cli/src/commands/start.ts packages/server/src/index.ts
git commit -m "feat(server,cli): add --verbose mode with request/event logging"
```

---

## Task 24: CLI — `--verbose` mode for `taskcast start` (Rust)

**Files:**
- Modify: `rust/taskcast-cli/src/commands/start.rs`
- Modify: `rust/taskcast-server/src/app.rs`
- Test: `rust/taskcast-server/tests/verbose_logger.rs`

Implement as an Axum `tower::Layer` middleware that logs the same format as the TypeScript version. Add `--verbose` flag to the start command args.

```bash
git commit -m "feat(server-rs,cli-rs): add --verbose mode with request/event logging"
```

---

## Task 25: Skill update — Add debugging and agent workflow sections

**Files:**
- Modify: `docs/skill/taskcast.md`

**Step 1: Read current skill file**

Read `docs/skill/taskcast.md` to understand current structure.

**Step 2: Append new sections**

Add three new sections at the end of the file:

### Section 1: Debugging

```markdown
## Debugging

### CLI Quick Checks

```bash
taskcast ping                          # Server reachable?
taskcast doctor                        # Storage + auth + connectivity
taskcast tasks list --status running   # Any stuck tasks?
taskcast tasks inspect <taskId>        # Full task details + recent events
taskcast logs <taskId>                 # Real-time event stream
taskcast tail                          # Watch all tasks globally
```

### Common Errors and Fixes

| Error | Cause | Fix |
|-------|-------|-----|
| `Task not found: <id>` | Task expired (TTL) or cleaned up | Check TTL settings; query long-term store if configured |
| `Cannot publish to task in terminal status` | Task already completed/failed/cancelled | Check task status before publishing; create a new task |
| `Invalid transition: pending → completed` | Must go through `running` first | `transitionTask(id, 'running')` then `transitionTask(id, 'completed')` |
| `403 Forbidden` | JWT missing required scope | Check token scopes; need `event:publish`, `task:create`, etc. |
| SSE connects but no events | Task still in `pending` status | Transition to `running` — SSE holds until task is running |
| `ECONNREFUSED` | Server not running or wrong port | Run `taskcast ping` to verify; check port with `taskcast doctor` |

### State Machine Reference

```
pending → running → completed | failed | timeout | cancelled
pending → assigned → running (with worker assignment)
pending → cancelled
running → paused → running (resumable)
running → blocked → running (after resolve)
```
```

### Section 2: Agent Workflow Patterns

```markdown
## Agent Workflow Patterns

### Agent as Producer (streaming output)

```typescript
const taskcast = new TaskcastServerClient({ baseUrl: 'http://localhost:3721' })

const task = await taskcast.createTask({ type: 'llm.chat', params: { prompt } })
await taskcast.transitionTask(task.id, 'running')

try {
  for await (const chunk of llmStream) {
    await taskcast.publishEvent(task.id, {
      type: 'llm.delta', level: 'info',
      data: { delta: chunk.text },
      seriesId: 'response', seriesMode: 'accumulate',
    })
  }
  await taskcast.transitionTask(task.id, 'completed', { result: { output: fullText } })
} catch (err) {
  await taskcast.transitionTask(task.id, 'failed', {
    error: { message: err.message, code: 'LLM_ERROR' },
  })
}
```

### Agent as Orchestrator (managing subtasks)

```typescript
const subtasks = await Promise.all(
  steps.map(step => taskcast.createTask({ type: 'agent.step', params: step }))
)

// Poll until all complete
const interval = setInterval(async () => {
  const results = await Promise.all(subtasks.map(t => taskcast.getTask(t.id)))
  const allDone = results.every(t => ['completed', 'failed'].includes(t.status))
  if (allDone) {
    clearInterval(interval)
    // Process results...
  }
}, 1000)
```
```

### Section 3: Node Management

```markdown
## Node Management (CLI)

```bash
# Add connections
taskcast node add local --url http://localhost:3721
taskcast node add prod --url https://tc.example.com --token <jwt> --token-type jwt
taskcast node add staging --url https://s.tc.io --token <admin-token> --token-type admin

# Switch default
taskcast node use prod

# All commands now target prod
taskcast tasks list
taskcast logs <taskId>

# Override per-command
taskcast tasks list --node local
```
```

**Step 3: Commit**

```bash
git add docs/skill/taskcast.md
git commit -m "docs(skill): add debugging, agent patterns, and node management sections"
```

---

## Task 26: Integration tests — Full CLI end-to-end (TypeScript)

**Files:**
- Create: `packages/cli/tests/integration/cli-e2e.test.ts`

Write end-to-end tests that start an in-memory Taskcast server (via `createTaskcastApp`), then test the full flow:

1. `ping` → OK
2. `doctor` → all adapters OK
3. `tasks list` → empty
4. Create a task (via engine directly), transition to running, publish events
5. `tasks list` → shows the task
6. `tasks inspect <id>` → shows task + events
7. `logs <id>` → receives events (test with completed task for deterministic output)

Use `app.request()` pattern for HTTP assertions rather than spawning child processes — this is faster and more reliable.

```bash
git commit -m "test(cli): add end-to-end integration tests for client commands"
```

---

## Task 27: Integration tests — Full CLI end-to-end (Rust)

**Files:**
- Create: `rust/taskcast-cli/tests/cli_e2e.rs`

Same test scenarios using `axum_test::TestServer` and the command functions directly (not subprocess).

```bash
git commit -m "test(cli-rs): add end-to-end integration tests for client commands"
```

---

## Summary

| Task | Description | TS/Rust |
|------|-------------|---------|
| 1-2 | `/health/detail` endpoint | Both |
| 3-4 | Global SSE `GET /events` | Both |
| 5-6 | Refactor CLI into command files | Both |
| 7-8 | Node config manager | Both |
| 9-10 | HTTP client helper | Both |
| 11-12 | `taskcast node` commands | Both |
| 13-14 | `taskcast ping` | Both |
| 15-16 | `taskcast doctor` | Both |
| 17-18 | `taskcast tasks list/inspect` | Both |
| 19-20 | `taskcast logs` | Both |
| 21-22 | `taskcast tail` | Both |
| 23-24 | `--verbose` mode | Both |
| 25 | Skill update | Docs |
| 26-27 | E2E integration tests | Both |
