# E2E Testing Design

Date: 2026-03-06

## Overview

Add three layers of testing that currently don't exist: API E2E tests, dashboard component tests, and browser E2E tests via Playwright. Run in a dedicated CI workflow on PRs and main merges only.

## Architecture

```
A. packages/e2e/              — API E2E (real TS server, HTTP calls)
B. packages/dashboard-web/tests/  — Component tests (vitest + jsdom + testing-library)
C. packages/e2e/browser/      — Browser E2E (Playwright + chromium)

CI: .github/workflows/e2e.yml — independent workflow, PR + main only
```

## A. API E2E (`packages/e2e/`)

**Tools**: vitest + `@hono/node-server` + fetch

**Server startup**: Each test file starts a real TS server in `beforeAll` using `createTaskcastApp` + `serve` on a random port. Memory adapters only — no Redis/Postgres needed.

**TS/Rust parity**: Environment variable `TASKCAST_TEST_URL` switches the target. Default: in-process TS server. CI adds a second run pointing at a Rust binary.

### Test cases

- **Task lifecycle**: create → publish events → SSE subscribe → transition → terminal
- **Admin auth flow**: POST /admin/token with admin token → receive JWT → use JWT on protected endpoints
- **Worker lifecycle**: register → heartbeat → task assignment → drain → resume → disconnect
- **SSE streaming**: replay history, live streaming, done event on terminal
- **Auth boundaries**: no token → 401, expired token → 401, insufficient scope → 403
- **Concurrency**: multiple SSE subscribers on same task, concurrent status transitions (only one succeeds)

## B. Dashboard Component Tests (`packages/dashboard-web/tests/`)

**Tools**: vitest + jsdom + `@testing-library/react` + `msw` (Mock Service Worker)

### Test scope

**Stores**:
- `connection.ts` — connect/disconnect, persist to localStorage, health check 401 handling

**Hooks**:
- `use-tasks` — query/mutation behavior with msw mocked API
- `use-workers` — worker list query, drain/disconnect mutations
- `use-stats` — derived stats computation, sorting, isPending propagation
- `use-events` — SSE stream accumulation, done state, error handling

**Key components**:
- TaskTable — row count, status badges, click selection
- WorkerTable — capacity progress bars, drain button
- TaskDetail — field display, action buttons
- Error Boundary — triggers fallback UI on render error

**Not tested here** (covered by Playwright):
- Pure display components (StatusCards, badges, sidebar)
- Page-level navigation and routing
- Visual layout and responsiveness

## C. Browser E2E (`packages/e2e/browser/`)

**Tools**: Playwright (chromium only)

**Server startup**: `beforeAll` starts TS server (port 3799, adminApi enabled, auth mode none) + dashboard via `taskcast ui --server http://localhost:3799 --port 3722`. `afterAll` kills both.

### Test cases

- **Login**: enter server URL + admin token → redirects to Overview
- **Overview page**: status cards visible, worker summary, recent tasks list
- **Tasks page**: create task → list refreshes → click row → detail panel → transition status
- **Events page**: select task → SSE events appear in real-time
- **Workers page**: worker list → drain worker → status changes
- **Navigation**: sidebar links switch pages, direct URL access works (SPA routing)

## CI (`.github/workflows/e2e.yml`)

```yaml
name: E2E Tests
on:
  pull_request:
  push:
    branches: [main]

jobs:
  api-e2e:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - run: pnpm install --frozen-lockfile
      - run: pnpm build
      - run: pnpm --filter @taskcast/e2e test

  dashboard-unit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - run: pnpm install --frozen-lockfile
      - run: pnpm build
      - run: pnpm --filter @taskcast/dashboard-web test

  browser-e2e:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - run: pnpm install --frozen-lockfile
      - run: pnpm build
      - run: npx playwright install --with-deps chromium
      - run: npx playwright test
        working-directory: packages/e2e
```

Three jobs run in parallel. Browser E2E installs only chromium (not all browsers) to keep CI fast.

## Package structure

```
packages/e2e/
  package.json          — deps: vitest, playwright, @taskcast/server, @taskcast/core
  vitest.config.ts      — for API E2E tests
  playwright.config.ts  — for browser E2E tests
  tests/
    api/
      task-lifecycle.test.ts
      admin-auth.test.ts
      worker-lifecycle.test.ts
      sse-streaming.test.ts
      auth-boundaries.test.ts
      concurrency.test.ts
    helpers/
      server.ts         — startServer/stopServer helpers
  browser/
    login.spec.ts
    overview.spec.ts
    tasks.spec.ts
    events.spec.ts
    workers.spec.ts
    navigation.spec.ts

packages/dashboard-web/tests/
  setup.ts              — msw server setup
  stores/
    connection.test.ts
  hooks/
    use-tasks.test.ts
    use-workers.test.ts
    use-stats.test.ts
    use-events.test.ts
  components/
    task-table.test.tsx
    worker-table.test.tsx
    task-detail.test.tsx
    error-boundary.test.tsx
```

## Dependencies to add

- `@playwright/test` — browser E2E (packages/e2e)
- `msw` — API mocking for dashboard component tests (packages/dashboard-web)
- `@testing-library/react`, `@testing-library/dom` — component rendering (packages/dashboard-web)
- `jsdom` — DOM environment (packages/dashboard-web vitest config)
