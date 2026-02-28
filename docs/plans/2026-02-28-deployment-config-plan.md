# Deployment Configuration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Dockerfile, docker-compose, Railway, and Fly.io configs so users can self-host Taskcast with one command.

**Architecture:** Multi-stage Docker build using `pnpm deploy` to extract @taskcast/cli with minimal production deps. docker-compose with profiles for graduated complexity (memory → redis → redis+pg). Platform configs for Railway and Fly.io point at the Dockerfile.

**Tech Stack:** Docker (multi-stage, node:22-alpine), docker-compose v3 profiles, Railway (railway.toml), Fly.io (fly.toml)

---

### Task 1: .dockerignore

**Files:**
- Create: `.dockerignore`

**Step 1: Create .dockerignore**

```
node_modules/
dist/
coverage/
.git/
.env
*.env.local
*.tsbuildinfo
.worktrees/
docs/
**/*.test.ts
**/*.spec.ts
**/tests/
**/vitest.config.ts
vitest.workspace.ts
README.md
README.zh.md
LICENSE
.dockerignore
Dockerfile
docker-compose.yml
fly.toml
railway.toml
```

**Step 2: Commit**

```bash
git add .dockerignore
git commit -m "chore: add .dockerignore"
```

---

### Task 2: Dockerfile

**Files:**
- Create: `Dockerfile`

**Context:** The monorepo uses pnpm workspaces. `@taskcast/cli` depends on `@taskcast/core`, `@taskcast/server`, `@taskcast/redis`, `@taskcast/postgres` via `workspace:*`. `pnpm deploy` resolves these into a flat standalone directory with real (non-symlinked) node_modules.

**Step 1: Create the Dockerfile**

```dockerfile
# ── Stage 1: base ──────────────────────────────────────────────
FROM node:22-alpine AS base
RUN corepack enable && corepack prepare pnpm@latest --activate
WORKDIR /app

# ── Stage 2: build ─────────────────────────────────────────────
FROM base AS build
COPY pnpm-lock.yaml pnpm-workspace.yaml package.json ./
COPY packages/ packages/
COPY tsconfig.base.json ./
RUN pnpm install --frozen-lockfile
RUN pnpm build

# ── Stage 3: deploy ────────────────────────────────────────────
FROM base AS deploy
COPY --from=build /app /app
RUN pnpm deploy --filter=@taskcast/cli --prod /prod

# ── Stage 4: runtime ───────────────────────────────────────────
FROM node:22-alpine AS runtime
WORKDIR /app
COPY --from=deploy /prod/node_modules ./node_modules
COPY --from=deploy /prod/dist ./dist
COPY --from=deploy /prod/package.json ./

ENV NODE_ENV=production
EXPOSE 3721
CMD ["node", "dist/index.js"]
```

**Step 2: Verify build locally (manual — skip in CI)**

```bash
docker build -t taskcast:local .
docker run --rm -p 3721:3721 taskcast:local
# Expected: "[taskcast] No TASKCAST_REDIS_URL configured — using in-memory adapters"
# Expected: "[taskcast] Server started on http://localhost:3721"
```

**Step 3: Commit**

```bash
git add Dockerfile
git commit -m "feat: add multi-stage Dockerfile for standalone deployment"
```

---

### Task 3: .env.example

**Files:**
- Create: `.env.example`

**Step 1: Create .env.example**

```bash
# Taskcast Configuration
# Copy this file to .env and fill in the values

# ── Server ──────────────────────────────────────────────────────
# Port for the Taskcast HTTP server (default: 3721)
TASKCAST_PORT=3721

# ── Redis (optional) ───────────────────────────────────────────
# Enables broadcast (pub/sub) and short-term event storage
# Leave empty to use in-memory adapters
TASKCAST_REDIS_URL=redis://redis:6379

# ── PostgreSQL (optional) ──────────────────────────────────────
# Enables long-term event persistence
# Leave empty to skip long-term storage
TASKCAST_POSTGRES_URL=postgres://taskcast:taskcast@postgres:5432/taskcast

# ── Auth ────────────────────────────────────────────────────────
# Authentication mode: none | jwt
TASKCAST_AUTH_MODE=none

# JWT secret (required when TASKCAST_AUTH_MODE=jwt)
# TASKCAST_JWT_SECRET=your-secret-here

# ── Logging ─────────────────────────────────────────────────────
TASKCAST_LOG_LEVEL=info

# ── Sentry (optional) ──────────────────────────────────────────
# SENTRY_DSN=https://xxx@sentry.io/yyy
```

**Step 2: Commit**

```bash
git add .env.example
git commit -m "chore: add .env.example with all configuration options"
```

---

### Task 4: docker-compose.yml

**Files:**
- Create: `docker-compose.yml`

**Context:** Use compose profiles so `docker compose up` runs taskcast-only (in-memory), `--profile redis` adds Redis, `--profile full` adds Redis + PostgreSQL. Services in the `full` profile must include Redis too (not just Postgres), since the full stack needs both.

**Step 1: Create docker-compose.yml**

```yaml
services:
  taskcast:
    build: .
    ports:
      - "${TASKCAST_PORT:-3721}:3721"
    environment:
      - TASKCAST_REDIS_URL=${TASKCAST_REDIS_URL:-}
      - TASKCAST_POSTGRES_URL=${TASKCAST_POSTGRES_URL:-}
      - TASKCAST_AUTH_MODE=${TASKCAST_AUTH_MODE:-none}
      - TASKCAST_JWT_SECRET=${TASKCAST_JWT_SECRET:-}
      - TASKCAST_LOG_LEVEL=${TASKCAST_LOG_LEVEL:-info}
    depends_on:
      redis:
        condition: service_started
        required: false
      postgres:
        condition: service_healthy
        required: false
    restart: unless-stopped

  redis:
    image: redis:7-alpine
    profiles: [redis, full]
    ports:
      - "6379:6379"
    volumes:
      - redis-data:/data
    restart: unless-stopped

  postgres:
    image: postgres:16-alpine
    profiles: [full]
    environment:
      POSTGRES_USER: taskcast
      POSTGRES_PASSWORD: taskcast
      POSTGRES_DB: taskcast
    ports:
      - "5432:5432"
    volumes:
      - pg-data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U taskcast"]
      interval: 5s
      timeout: 3s
      retries: 5
    restart: unless-stopped

volumes:
  redis-data:
  pg-data:
```

**Step 2: Verify profiles (manual)**

```bash
# Memory only
docker compose config
# Should show only taskcast service

# With Redis
docker compose --profile redis config
# Should show taskcast + redis

# Full stack
docker compose --profile full config
# Should show taskcast + redis + postgres
```

**Step 3: Commit**

```bash
git add docker-compose.yml
git commit -m "feat: add docker-compose with profiles (minimal/redis/full)"
```

---

### Task 5: railway.toml

**Files:**
- Create: `railway.toml`

**Step 1: Create railway.toml**

```toml
[build]
builder = "dockerfile"
dockerfilePath = "Dockerfile"

[deploy]
startCommand = "node dist/index.js"
healthcheckPath = "/tasks"
healthcheckTimeout = 5
restartPolicyType = "on_failure"
restartPolicyMaxRetries = 3
```

**Step 2: Commit**

```bash
git add railway.toml
git commit -m "feat: add Railway deployment config"
```

---

### Task 6: fly.toml

**Files:**
- Create: `fly.toml`

**Step 1: Create fly.toml**

```toml
app = "taskcast"
primary_region = "nrt"

[build]
  dockerfile = "Dockerfile"

[env]
  TASKCAST_LOG_LEVEL = "info"

[http_service]
  internal_port = 3721
  force_https = true
  auto_stop_machines = "stop"
  auto_start_machines = true
  min_machines_running = 0

[[http_service.checks]]
  grace_period = "10s"
  interval = "30s"
  method = "GET"
  timeout = "5s"
  path = "/tasks"
```

**Step 2: Commit**

```bash
git add fly.toml
git commit -m "feat: add Fly.io deployment config"
```

---

### Task 7: Verify Docker build

**Step 1: Build the image**

```bash
docker build -t taskcast:test .
```

Expected: Build succeeds, final image is < 200MB.

**Step 2: Run smoke test (in-memory mode)**

```bash
docker run --rm -d --name taskcast-test -p 3721:3721 taskcast:test
sleep 2
curl -s http://localhost:3721/tasks | head -c 200
docker stop taskcast-test
```

Expected: HTTP 200 response (empty array or valid JSON).

**Step 3: Run smoke test (full stack)**

```bash
docker compose --profile full up -d
sleep 5
curl -s http://localhost:3721/tasks | head -c 200
docker compose --profile full down -v
```

Expected: HTTP 200 response with Redis + PostgreSQL connected (no warning about in-memory).

**Step 4: Squash into final commit (if all tests pass)**

If prior commits were granular, optionally squash. Otherwise leave as-is — granular history is fine.
