# @taskcast/dashboard-web Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a production-ready management dashboard SPA for monitoring and managing Taskcast servers.

**Architecture:** Pure client-side SPA (React + Vite) connecting to a remote Taskcast server via REST + SSE. Zustand for UI state, TanStack Query for server state. Deployable via CLI serve, Docker, or static CDN. Server-side prerequisites (admin token exchange, hot/cold fields, worker drain API) are implemented first in both TypeScript and Rust.

**Tech Stack:** React 18, Vite, shadcn/ui, Tailwind CSS, Zustand, TanStack Query, `@taskcast/server-sdk`, `@taskcast/client`, react-router-dom

**Design Doc:** `docs/plans/2026-03-06-dashboard-web-design.md`

---

## Phase 1: Server-Side Prerequisites

Server changes are needed before the dashboard can function. Each change must be made in both TypeScript (`packages/server`) and Rust (`rust/taskcast-server`) implementations simultaneously per CLAUDE.md rules.

### Task 1: Admin Token Config

Add `adminToken` field to server config. If not set, auto-generate UUID on startup and print to terminal.

**Files:**
- Modify: `packages/core/src/types.ts` — add `adminToken` to config types
- Modify: `packages/core/src/config.ts` — handle adminToken generation
- Modify: `rust/taskcast-core/src/types.rs` — Rust config types
- Test: `packages/core/tests/unit/config.test.ts`

**Step 1: Add adminToken to TypeScript types**

In `packages/core/src/types.ts`, find `TaskcastConfig` interface and add:

```typescript
adminToken?: string
```

**Step 2: Add adminToken generation logic in config.ts**

In `packages/core/src/config.ts`, after config is loaded, if `adminToken` is not set, generate a UUID:

```typescript
import { ulid } from 'ulidx'

// In resolveConfig or wherever config is finalized:
if (!config.adminToken) {
  config.adminToken = ulid()
  console.log(`[taskcast] Admin token (auto-generated): ${config.adminToken}`)
}
```

**Step 3: Add to Rust types**

In `rust/taskcast-core/src/types.rs`, add `admin_token: Option<String>` to the config struct. In startup logic, generate UUID if not set using `ulid` crate.

**Step 4: Write test for auto-generation**

```typescript
// packages/core/tests/unit/config.test.ts
it('should auto-generate adminToken if not provided', () => {
  const config = resolveConfig({})
  expect(config.adminToken).toBeDefined()
  expect(config.adminToken.length).toBeGreaterThan(0)
})

it('should use provided adminToken', () => {
  const config = resolveConfig({ adminToken: 'my-secret' })
  expect(config.adminToken).toBe('my-secret')
})
```

**Step 5: Run tests**

```bash
cd packages/core && pnpm test
```

**Step 6: Commit**

```bash
git add packages/core/src/types.ts packages/core/src/config.ts packages/core/tests/unit/config.test.ts rust/taskcast-core/src/types.rs
git commit -m "feat(core): add adminToken config with auto-generation"
```

---

### Task 2: POST /admin/token Route

Exchange admin token for JWT. New route on server.

**Files:**
- Create: `packages/server/src/routes/admin.ts`
- Modify: `packages/server/src/index.ts` — mount admin route
- Modify: `rust/taskcast-server/src/app.rs` — add Rust admin route
- Test: `packages/server/tests/admin-token.test.ts`

**Step 1: Write the failing test**

```typescript
// packages/server/tests/admin-token.test.ts
import { describe, it, expect } from 'vitest'
import { createTaskcastApp } from '../src/index.js'
import { MemoryShortTermStore, MemoryBroadcastProvider } from '@taskcast/core'

describe('POST /admin/token', () => {
  const adminToken = 'test-admin-token'

  function createApp() {
    return createTaskcastApp({
      shortTermStore: new MemoryShortTermStore(),
      broadcast: new MemoryBroadcastProvider(),
      adminToken,
      auth: { mode: 'jwt', jwt: { algorithm: 'HS256', secret: 'test-secret-key-for-jwt-signing-min-32-chars' } },
    })
  }

  it('should return JWT when valid admin token is provided', async () => {
    const app = createApp()
    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeDefined()
    expect(typeof body.token).toBe('string')
    expect(body.expiresAt).toBeDefined()
  })

  it('should reject invalid admin token', async () => {
    const app = createApp()
    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'wrong-token' }),
    })
    expect(res.status).toBe(401)
  })

  it('should accept custom scopes', async () => {
    const app = createApp()
    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken, scopes: ['event:subscribe', 'event:history'] }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeDefined()
  })

  it('should return 404 when auth mode is not jwt', async () => {
    const app = createTaskcastApp({
      shortTermStore: new MemoryShortTermStore(),
      broadcast: new MemoryBroadcastProvider(),
      adminToken,
      auth: { mode: 'none' },
    })
    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken }),
    })
    // When auth mode is 'none', admin token endpoint isn't needed
    // but should still work - return a dummy token or 200 with no token
    // Actually, in 'none' mode, no auth needed at all, so endpoint can return 404 or just succeed
    expect(res.status).toBe(200)
  })
})
```

**Step 2: Run test to verify it fails**

```bash
cd packages/server && pnpm test tests/admin-token.test.ts
```

Expected: FAIL (route does not exist yet)

**Step 3: Implement admin route**

```typescript
// packages/server/src/routes/admin.ts
import { Hono } from 'hono'
import { z } from 'zod'
import * as jose from 'jose'
import type { TaskcastAppEnv } from '../index.js'

const AdminTokenRequest = z.object({
  adminToken: z.string(),
  scopes: z.array(z.string()).optional(),
  expiresIn: z.number().optional().default(86400), // 24h
})

export function createAdminRoutes() {
  const router = new Hono<TaskcastAppEnv>()

  router.post('/token', async (c) => {
    const config = c.get('config')
    const body = AdminTokenRequest.parse(await c.req.json())

    if (body.adminToken !== config.adminToken) {
      return c.json({ error: 'Invalid admin token' }, 401)
    }

    const authConfig = config.auth
    if (authConfig?.mode === 'jwt' && authConfig.jwt) {
      const scopes = body.scopes ?? ['*']
      const expiresAt = Date.now() + body.expiresIn * 1000

      // Sign JWT with same key used by auth middleware
      const secret = new TextEncoder().encode(authConfig.jwt.secret)
      const token = await new jose.SignJWT({
        scope: scopes,
        taskIds: '*',
      })
        .setProtectedHeader({ alg: authConfig.jwt.algorithm ?? 'HS256' })
        .setIssuedAt()
        .setExpirationTime(Math.floor(expiresAt / 1000))
        .setSubject('admin')
        .sign(secret)

      return c.json({ token, expiresAt })
    }

    // Non-JWT mode: return a placeholder (no auth needed)
    return c.json({ token: '', expiresAt: 0 })
  })

  return router
}
```

**Step 4: Mount in index.ts**

In `packages/server/src/index.ts`, import and mount:

```typescript
import { createAdminRoutes } from './routes/admin.js'

// Inside createTaskcastApp, after other routes:
app.route('/admin', createAdminRoutes())
```

Also ensure `config` (with `adminToken`) is set via `c.set('config', options)` in middleware.

**Step 5: Implement Rust equivalent**

In `rust/taskcast-server/src/app.rs`, add `POST /admin/token` handler that validates admin token and signs JWT using the same JWT config.

**Step 6: Run tests**

```bash
cd packages/server && pnpm test tests/admin-token.test.ts
```

Expected: PASS

**Step 7: Commit**

```bash
git add packages/server/src/routes/admin.ts packages/server/src/index.ts packages/server/tests/admin-token.test.ts rust/taskcast-server/src/app.rs
git commit -m "feat(server): add POST /admin/token for dashboard auth"
```

---

### Task 3: Task Hot/Cold + Subscriber Count Fields

Add `hot` and `subscriberCount` to task responses.

**Files:**
- Modify: `packages/core/src/types.ts` — add response fields
- Modify: `packages/server/src/routes/tasks.ts` — enrich GET responses
- Modify: `packages/server/src/routes/sse.ts` — track subscriber count
- Modify: `rust/taskcast-core/src/types.rs`
- Modify: `rust/taskcast-server/src/app.rs`
- Test: `packages/server/tests/task-enrichment.test.ts`

**Step 1: Write the failing test**

```typescript
// packages/server/tests/task-enrichment.test.ts
import { describe, it, expect } from 'vitest'
import { createTaskcastApp } from '../src/index.js'
import { MemoryShortTermStore, MemoryBroadcastProvider } from '@taskcast/core'

describe('Task enrichment fields', () => {
  function createApp() {
    return createTaskcastApp({
      shortTermStore: new MemoryShortTermStore(),
      broadcast: new MemoryBroadcastProvider(),
    })
  }

  it('GET /tasks/:id should include hot and subscriberCount', async () => {
    const app = createApp()

    // Create a task
    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })
    const task = await createRes.json()

    // Get task - should have hot/subscriberCount
    const res = await app.request(`/tasks/${task.id}`)
    const enriched = await res.json()
    expect(enriched.hot).toBe(true) // Just created, still in ShortTermStore
    expect(enriched.subscriberCount).toBe(0) // No SSE subscribers
  })
})
```

**Step 2: Run test to verify it fails**

```bash
cd packages/server && pnpm test tests/task-enrichment.test.ts
```

**Step 3: Implement subscriber tracking**

In `packages/server/src/routes/sse.ts`, add a shared map to track active subscribers per task:

```typescript
// At module level or passed via context
const subscriberCounts = new Map<string, number>()

export function getSubscriberCount(taskId: string): number {
  return subscriberCounts.get(taskId) ?? 0
}

// In SSE handler, when subscription starts:
subscriberCounts.set(taskId, (subscriberCounts.get(taskId) ?? 0) + 1)

// In cleanup (stream close):
const count = subscriberCounts.get(taskId) ?? 1
if (count <= 1) subscriberCounts.delete(taskId)
else subscriberCounts.set(taskId, count - 1)
```

**Step 4: Enrich task responses**

In `packages/server/src/routes/tasks.ts`, for `GET /tasks/:id` and any task list endpoint:

```typescript
import { getSubscriberCount } from './sse.js'

// After fetching task from engine:
const enrichedTask = {
  ...task,
  hot: true, // If we got it from engine.getTask(), it's in ShortTermStore
  subscriberCount: getSubscriberCount(task.id),
}
return c.json(enrichedTask)
```

Note: `hot` is always `true` when fetched from ShortTermStore. If a `LongTermStore` query is added later, tasks from there would be `hot: false`.

**Step 5: Add Rust implementation**

Mirror the subscriber tracking and task enrichment in Rust.

**Step 6: Run tests**

```bash
cd packages/server && pnpm test tests/task-enrichment.test.ts
```

**Step 7: Commit**

```bash
git add packages/server/src/routes/tasks.ts packages/server/src/routes/sse.ts packages/server/tests/task-enrichment.test.ts packages/core/src/types.ts rust/
git commit -m "feat(server): add hot/subscriberCount to task responses"
```

---

### Task 4: Worker Drain API

Add `PATCH /workers/:id/status` to set worker status (e.g., draining).

**Files:**
- Modify: `packages/server/src/routes/workers.ts`
- Modify: `packages/core/src/worker-manager.ts` — add setWorkerStatus method
- Modify: `rust/taskcast-core/src/worker_manager.rs`
- Modify: `rust/taskcast-server/src/app.rs`
- Test: `packages/server/tests/worker-drain.test.ts`

**Step 1: Write the failing test**

```typescript
// packages/server/tests/worker-drain.test.ts
import { describe, it, expect } from 'vitest'
// Setup with worker manager enabled, register a worker, then drain it

describe('PATCH /workers/:id/status', () => {
  it('should set worker to draining', async () => {
    // Create app with worker manager
    // Register worker via WS or mock
    // PATCH /workers/:workerId/status { status: 'draining' }
    // Expect 200, worker status = draining
    // GET /workers/:workerId → status should be 'draining'
  })

  it('should resume worker from draining to idle', async () => {
    // PATCH /workers/:workerId/status { status: 'idle' }
    // Expect 200
  })

  it('should reject invalid status transitions', async () => {
    // PATCH with status: 'busy' → 400 (can't manually set to busy)
  })
})
```

**Step 2: Implement in worker-manager.ts**

Add a `setWorkerStatus(workerId: string, status: 'draining' | 'idle')` method to WorkerManager.

**Step 3: Add route in workers.ts**

```typescript
router.patch('/:workerId/status', async (c) => {
  const { workerId } = c.req.param()
  const { status } = await c.req.json()
  // Validate: only 'draining' and 'idle' allowed via API
  const worker = await workerManager.setWorkerStatus(workerId, status)
  return c.json(worker)
})
```

**Step 4: Add Rust implementation**

**Step 5: Run tests, commit**

```bash
git commit -m "feat(server): add PATCH /workers/:id/status for drain control"
```

---

### Task 5: GET /tasks List Endpoint

The server currently only has `GET /tasks/:id` for individual tasks. The dashboard needs a list endpoint.

**Files:**
- Modify: `packages/server/src/routes/tasks.ts` — add `GET /tasks`
- Modify: `packages/core/src/types.ts` — ensure `listTasks` filter types exist
- Modify: `rust/taskcast-server/src/app.rs`
- Test: `packages/server/tests/task-list.test.ts`

**Step 1: Check if GET /tasks already exists**

Look at `packages/server/src/routes/tasks.ts` — if `listTasks` is already exposed via `ShortTermStore.listTasks()`, wire it up. If not, add it.

**Step 2: Write test**

```typescript
describe('GET /tasks', () => {
  it('should return all tasks', async () => {
    const app = createApp()
    // Create 3 tasks
    await app.request('/tasks', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ type: 'a' }) })
    await app.request('/tasks', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ type: 'b' }) })
    await app.request('/tasks', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ type: 'c' }) })

    const res = await app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks.length).toBe(3)
  })

  it('should filter by status', async () => {
    // GET /tasks?status=running
  })

  it('should filter by type', async () => {
    // GET /tasks?type=llm.*
  })
})
```

**Step 3: Implement route**

```typescript
router.get('/', async (c) => {
  const auth = c.get('auth')
  checkScope(auth, 'event:subscribe')

  const status = c.req.query('status')
  const type = c.req.query('type')
  const tags = c.req.query('tags')

  const filter: TaskFilter = {}
  if (status) filter.status = status.split(',') as TaskStatus[]
  if (type) filter.type = type
  if (tags) filter.tags = tags.split(',')

  const tasks = await engine.listTasks(filter)
  const enriched = tasks.map(t => ({
    ...t,
    hot: true,
    subscriberCount: getSubscriberCount(t.id),
  }))

  return c.json({ tasks: enriched })
})
```

**Step 4: Implement in Rust, run tests, commit**

```bash
git commit -m "feat(server): add GET /tasks list endpoint with filters"
```

---

## Phase 2: Dashboard Package Setup

### Task 6: Package Scaffolding

Create the `packages/dashboard-web` package with Vite + React + Tailwind + shadcn/ui.

**Files:**
- Create: `packages/dashboard-web/package.json`
- Create: `packages/dashboard-web/vite.config.ts`
- Create: `packages/dashboard-web/tsconfig.json`
- Create: `packages/dashboard-web/tsconfig.node.json`
- Create: `packages/dashboard-web/tailwind.config.ts`
- Create: `packages/dashboard-web/postcss.config.js`
- Create: `packages/dashboard-web/index.html`
- Create: `packages/dashboard-web/src/main.tsx`
- Create: `packages/dashboard-web/src/App.tsx`
- Create: `packages/dashboard-web/src/index.css`
- Create: `packages/dashboard-web/src/lib/utils.ts`
- Create: `packages/dashboard-web/components.json` — shadcn config
- Modify: `pnpm-workspace.yaml` — ensure dashboard-web is included

**Step 1: Create package.json**

```json
{
  "name": "@taskcast/dashboard-web",
  "version": "0.3.0",
  "type": "module",
  "private": false,
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "preview": "vite preview"
  },
  "dependencies": {
    "@taskcast/client": "workspace:*",
    "@taskcast/core": "workspace:*",
    "@taskcast/server-sdk": "workspace:*",
    "react": "^18.3.0",
    "react-dom": "^18.3.0",
    "react-router-dom": "^7.0.0",
    "zustand": "^5.0.0",
    "@tanstack/react-query": "^5.0.0"
  },
  "devDependencies": {
    "@types/react": "^18.3.0",
    "@types/react-dom": "^18.3.0",
    "@vitejs/plugin-react": "^4.0.0",
    "tailwindcss": "^4.0.0",
    "@tailwindcss/vite": "^4.0.0",
    "typescript": "^5.7.0",
    "vite": "^6.0.0"
  }
}
```

**Step 2: Create vite.config.ts**

```typescript
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'path'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    outDir: 'dist',
  },
})
```

**Step 3: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "baseUrl": ".",
    "paths": {
      "@/*": ["./src/*"]
    }
  },
  "include": ["src"]
}
```

**Step 4: Create index.html**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Taskcast Dashboard</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

**Step 5: Create src/main.tsx and src/App.tsx**

```tsx
// src/main.tsx
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import './index.css'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
```

```tsx
// src/App.tsx
export function App() {
  return <div>Taskcast Dashboard</div>
}
```

**Step 6: Create src/index.css with Tailwind**

```css
@import "tailwindcss";
```

**Step 7: Create src/lib/utils.ts (shadcn requirement)**

```typescript
import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
```

Add `clsx` and `tailwind-merge` to dependencies.

**Step 8: Create components.json for shadcn**

```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "new-york",
  "rsc": false,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "src/index.css",
    "baseColor": "neutral",
    "cssVariables": true
  },
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui",
    "lib": "@/lib",
    "hooks": "@/hooks"
  }
}
```

**Step 9: Install deps and add shadcn components**

```bash
cd packages/dashboard-web && pnpm install
npx shadcn@latest add button card badge table tabs input select dialog sheet scroll-area progress dropdown-menu separator skeleton
```

**Step 10: Verify dev server starts**

```bash
cd packages/dashboard-web && pnpm dev
```

Expected: Vite dev server on localhost:5173, page shows "Taskcast Dashboard"

**Step 11: Commit**

```bash
git add packages/dashboard-web/
git commit -m "feat(dashboard-web): scaffold package with Vite + React + shadcn/ui"
```

---

### Task 7: Router + Layout Shell

Set up react-router-dom with sidebar navigation and page layout.

**Files:**
- Create: `packages/dashboard-web/src/components/layout/shell.tsx`
- Create: `packages/dashboard-web/src/components/layout/sidebar.tsx`
- Create: `packages/dashboard-web/src/components/layout/header.tsx`
- Create: `packages/dashboard-web/src/pages/overview.tsx`
- Create: `packages/dashboard-web/src/pages/tasks.tsx`
- Create: `packages/dashboard-web/src/pages/events.tsx`
- Create: `packages/dashboard-web/src/pages/workers.tsx`
- Modify: `packages/dashboard-web/src/App.tsx`

**Step 1: Create layout shell**

```tsx
// src/components/layout/shell.tsx
import { Sidebar } from './sidebar'
import { Header } from './header'

export function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex h-screen">
      <Sidebar />
      <div className="flex flex-1 flex-col overflow-hidden">
        <Header />
        <main className="flex-1 overflow-auto p-6">{children}</main>
      </div>
    </div>
  )
}
```

**Step 2: Create sidebar with nav links**

```tsx
// src/components/layout/sidebar.tsx
import { NavLink } from 'react-router-dom'
import { cn } from '@/lib/utils'

const navItems = [
  { to: '/', label: 'Overview', icon: 'LayoutDashboard' },
  { to: '/tasks', label: 'Tasks', icon: 'ListTodo' },
  { to: '/events', label: 'Events', icon: 'Radio' },
  { to: '/workers', label: 'Workers', icon: 'Cpu' },
]

export function Sidebar() {
  return (
    <aside className="w-56 border-r bg-muted/40 p-4">
      <h1 className="mb-6 text-lg font-semibold">Taskcast</h1>
      <nav className="space-y-1">
        {navItems.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            className={({ isActive }) =>
              cn(
                'block rounded-md px-3 py-2 text-sm font-medium',
                isActive ? 'bg-primary text-primary-foreground' : 'hover:bg-muted',
              )
            }
          >
            {item.label}
          </NavLink>
        ))}
      </nav>
    </aside>
  )
}
```

**Step 3: Create header**

```tsx
// src/components/layout/header.tsx
import { useConnectionStore } from '@/stores/connection'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'

export function Header() {
  const { baseUrl, connected, disconnect } = useConnectionStore()

  return (
    <header className="flex items-center justify-between border-b px-6 py-3">
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">Server:</span>
        <code className="text-sm">{baseUrl}</code>
        <Badge variant={connected ? 'default' : 'destructive'}>
          {connected ? 'Connected' : 'Disconnected'}
        </Badge>
      </div>
      {connected && (
        <Button variant="outline" size="sm" onClick={disconnect}>
          Disconnect
        </Button>
      )}
    </header>
  )
}
```

**Step 4: Create placeholder pages**

```tsx
// src/pages/overview.tsx
export function OverviewPage() {
  return <div><h2 className="text-2xl font-bold">Overview</h2></div>
}

// src/pages/tasks.tsx
export function TasksPage() {
  return <div><h2 className="text-2xl font-bold">Tasks</h2></div>
}

// src/pages/events.tsx
export function EventsPage() {
  return <div><h2 className="text-2xl font-bold">Events</h2></div>
}

// src/pages/workers.tsx
export function WorkersPage() {
  return <div><h2 className="text-2xl font-bold">Workers</h2></div>
}
```

**Step 5: Update App.tsx with router**

```tsx
// src/App.tsx
import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { Shell } from './components/layout/shell'
import { OverviewPage } from './pages/overview'
import { TasksPage } from './pages/tasks'
import { EventsPage } from './pages/events'
import { WorkersPage } from './pages/workers'
import { LoginPage } from './pages/login'
import { useConnectionStore } from './stores/connection'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchInterval: 5000,
      retry: 1,
    },
  },
})

export function App() {
  const connected = useConnectionStore((s) => s.connected)

  if (!connected) {
    return <LoginPage />
  }

  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Shell>
          <Routes>
            <Route path="/" element={<OverviewPage />} />
            <Route path="/tasks" element={<TasksPage />} />
            <Route path="/tasks/:taskId" element={<TasksPage />} />
            <Route path="/events" element={<EventsPage />} />
            <Route path="/workers" element={<WorkersPage />} />
          </Routes>
        </Shell>
      </BrowserRouter>
    </QueryClientProvider>
  )
}
```

**Step 6: Verify routing works**

```bash
cd packages/dashboard-web && pnpm dev
```

Navigate to /, /tasks, /events, /workers — each should render its placeholder.

**Step 7: Commit**

```bash
git commit -m "feat(dashboard-web): add router + layout shell with sidebar"
```

---

## Phase 3: Connection & Auth

### Task 8: Connection Store + Login Page

Zustand store for connection state and a login page with admin token input.

**Files:**
- Create: `packages/dashboard-web/src/stores/connection.ts`
- Create: `packages/dashboard-web/src/pages/login.tsx`
- Create: `packages/dashboard-web/src/lib/api.ts`

**Step 1: Create connection store**

```typescript
// src/stores/connection.ts
import { create } from 'zustand'
import { persist } from 'zustand/middleware'

interface ConnectionState {
  baseUrl: string
  jwt: string | null
  connected: boolean
  error: string | null
  connect: (url: string, adminToken: string) => Promise<void>
  disconnect: () => void
  setAutoConnect: (baseUrl: string, jwt: string) => void
}

export const useConnectionStore = create<ConnectionState>()(
  persist(
    (set, get) => ({
      baseUrl: '',
      jwt: null,
      connected: false,
      error: null,

      connect: async (url: string, adminToken: string) => {
        try {
          set({ error: null })

          // 1. Health check
          const healthRes = await fetch(`${url}/health`)
          if (!healthRes.ok) throw new Error('Server unreachable')

          // 2. Exchange admin token for JWT
          const tokenRes = await fetch(`${url}/admin/token`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ adminToken }),
          })

          if (tokenRes.status === 401) {
            throw new Error('Invalid admin token')
          }
          if (!tokenRes.ok) {
            throw new Error(`Token exchange failed: ${tokenRes.status}`)
          }

          const { token: jwt } = await tokenRes.json()
          set({ baseUrl: url, jwt, connected: true, error: null })
        } catch (err) {
          set({ connected: false, error: err instanceof Error ? err.message : String(err) })
          throw err
        }
      },

      disconnect: () => {
        set({ jwt: null, connected: false, error: null })
      },

      setAutoConnect: (baseUrl: string, jwt: string) => {
        set({ baseUrl, jwt, connected: true, error: null })
      },
    }),
    {
      name: 'taskcast-dashboard-connection',
      partialize: (state) => ({ baseUrl: state.baseUrl }),
      // Only persist baseUrl, not JWT (security)
    },
  ),
)
```

**Step 2: Create login page**

```tsx
// src/pages/login.tsx
import { useState } from 'react'
import { useConnectionStore } from '@/stores/connection'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

export function LoginPage() {
  const { connect, error, baseUrl: savedUrl } = useConnectionStore()
  const [url, setUrl] = useState(savedUrl || 'http://localhost:3721')
  const [adminToken, setAdminToken] = useState('')
  const [loading, setLoading] = useState(false)

  const handleConnect = async () => {
    setLoading(true)
    try {
      await connect(url, adminToken)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted/40">
      <Card className="w-96">
        <CardHeader>
          <CardTitle>Taskcast Dashboard</CardTitle>
          <CardDescription>Connect to a Taskcast server</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium">Server URL</label>
            <Input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="http://localhost:3721"
            />
          </div>
          <div className="space-y-2">
            <label className="text-sm font-medium">Admin Token</label>
            <Input
              type="password"
              value={adminToken}
              onChange={(e) => setAdminToken(e.target.value)}
              placeholder="Enter admin token"
            />
          </div>
          {error && <p className="text-sm text-destructive">{error}</p>}
          <Button onClick={handleConnect} disabled={loading} className="w-full">
            {loading ? 'Connecting...' : 'Connect'}
          </Button>
        </CardContent>
      </Card>
    </div>
  )
}
```

**Step 3: Create API client wrapper**

```typescript
// src/lib/api.ts
import { TaskcastServerClient } from '@taskcast/server-sdk'
import { useConnectionStore } from '@/stores/connection'

export function getApiClient(): TaskcastServerClient {
  const { baseUrl, jwt } = useConnectionStore.getState()
  return new TaskcastServerClient({
    baseUrl,
    token: jwt ?? undefined,
  })
}
```

**Step 4: Check for CLI auto-connect**

In `src/main.tsx`, before rendering, check if `GET /api/config` returns auto-connect info:

```typescript
// In main.tsx, before createRoot:
async function checkAutoConnect() {
  try {
    const res = await fetch('/api/config')
    if (res.ok) {
      const config = await res.json()
      if (config.baseUrl) {
        useConnectionStore.getState().setAutoConnect(config.baseUrl, config.token ?? '')
      }
    }
  } catch {
    // Not running in CLI mode, ignore
  }
}
await checkAutoConnect()
```

**Step 5: Verify login flow works**

Start a Taskcast server, then start dashboard dev server. Login with admin token should connect.

**Step 6: Commit**

```bash
git commit -m "feat(dashboard-web): add connection store + login page + API wrapper"
```

---

### Task 9: TanStack Query Hooks

Create query hooks for tasks, workers, and events.

**Files:**
- Create: `packages/dashboard-web/src/hooks/use-tasks.ts`
- Create: `packages/dashboard-web/src/hooks/use-workers.ts`
- Create: `packages/dashboard-web/src/hooks/use-events.ts`
- Create: `packages/dashboard-web/src/hooks/use-stats.ts`

**Step 1: Task hooks**

```typescript
// src/hooks/use-tasks.ts
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { getApiClient } from '@/lib/api'
import type { TaskStatus } from '@taskcast/core'

interface TaskFilter {
  status?: string
  type?: string
}

export function useTasksQuery(filter?: TaskFilter) {
  const client = getApiClient()
  return useQuery({
    queryKey: ['tasks', filter],
    queryFn: async () => {
      const params = new URLSearchParams()
      if (filter?.status) params.set('status', filter.status)
      if (filter?.type) params.set('type', filter.type)

      const res = await fetch(`${client.baseUrl}/tasks?${params}`, {
        headers: client.token ? { Authorization: `Bearer ${client.token}` } : {},
      })
      if (!res.ok) throw new Error(`Failed to fetch tasks: ${res.status}`)
      const body = await res.json()
      return body.tasks
    },
    refetchInterval: 3000,
  })
}

export function useTaskQuery(taskId: string | null) {
  const client = getApiClient()
  return useQuery({
    queryKey: ['task', taskId],
    queryFn: () => client.getTask(taskId!),
    enabled: !!taskId,
    refetchInterval: 3000,
  })
}

export function useCreateTask() {
  const client = getApiClient()
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: { type?: string; params?: Record<string, unknown>; ttl?: number; tags?: string[] }) =>
      client.createTask(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['tasks'] }),
  })
}

export function useTransitionTask() {
  const client = getApiClient()
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ taskId, status, payload }: { taskId: string; status: TaskStatus; payload?: Record<string, unknown> }) =>
      client.transitionTask(taskId, status, payload),
    onSuccess: (_, { taskId }) => {
      queryClient.invalidateQueries({ queryKey: ['tasks'] })
      queryClient.invalidateQueries({ queryKey: ['task', taskId] })
    },
  })
}
```

**Step 2: Worker hooks**

```typescript
// src/hooks/use-workers.ts
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { useConnectionStore } from '@/stores/connection'

export function useWorkersQuery() {
  const { baseUrl, jwt } = useConnectionStore()
  return useQuery({
    queryKey: ['workers'],
    queryFn: async () => {
      const res = await fetch(`${baseUrl}/workers`, {
        headers: jwt ? { Authorization: `Bearer ${jwt}` } : {},
      })
      if (!res.ok) throw new Error(`Failed to fetch workers: ${res.status}`)
      return res.json()
    },
    refetchInterval: 5000,
  })
}

export function useDrainWorker() {
  const { baseUrl, jwt } = useConnectionStore()
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async ({ workerId, status }: { workerId: string; status: 'draining' | 'idle' }) => {
      const res = await fetch(`${baseUrl}/workers/${workerId}/status`, {
        method: 'PATCH',
        headers: {
          'Content-Type': 'application/json',
          ...(jwt ? { Authorization: `Bearer ${jwt}` } : {}),
        },
        body: JSON.stringify({ status }),
      })
      if (!res.ok) throw new Error(`Failed to update worker: ${res.status}`)
      return res.json()
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['workers'] }),
  })
}

export function useDisconnectWorker() {
  const { baseUrl, jwt } = useConnectionStore()
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (workerId: string) => {
      const res = await fetch(`${baseUrl}/workers/${workerId}`, {
        method: 'DELETE',
        headers: jwt ? { Authorization: `Bearer ${jwt}` } : {},
      })
      if (!res.ok) throw new Error(`Failed to disconnect worker: ${res.status}`)
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['workers'] }),
  })
}
```

**Step 3: Event hooks**

```typescript
// src/hooks/use-events.ts
import { useQuery } from '@tanstack/react-query'
import { useConnectionStore } from '@/stores/connection'
import { useTaskEvents } from '@taskcast/react'
import type { SSEEnvelope } from '@taskcast/core'

export function useEventHistory(taskId: string | null) {
  const { baseUrl, jwt } = useConnectionStore()
  const client = getApiClient()
  return useQuery({
    queryKey: ['events', taskId],
    queryFn: () => client.getHistory(taskId!),
    enabled: !!taskId,
  })
}

export function useEventStream(taskId: string | null, filter?: { types?: string; levels?: string }) {
  const { baseUrl, jwt } = useConnectionStore()
  return useTaskEvents(taskId ?? '', {
    baseUrl,
    token: jwt ?? undefined,
    filter,
    enabled: !!taskId,
  })
}
```

**Step 4: Stats hook for overview**

```typescript
// src/hooks/use-stats.ts
import { useMemo } from 'react'
import { useTasksQuery } from './use-tasks'
import { useWorkersQuery } from './use-workers'
import type { Task, Worker } from '@taskcast/core'

export function useStats() {
  const { data: tasks = [] } = useTasksQuery()
  const { data: workers = [] } = useWorkersQuery()

  return useMemo(() => {
    const statusCounts: Record<string, number> = {}
    for (const task of tasks) {
      statusCounts[task.status] = (statusCounts[task.status] ?? 0) + 1
    }

    const totalCapacity = workers.reduce((sum: number, w: Worker) => sum + w.capacity, 0)
    const usedCapacity = workers.reduce((sum: number, w: Worker) => sum + w.usedSlots, 0)
    const onlineWorkers = workers.filter((w: Worker) => w.status !== 'offline').length

    return {
      statusCounts,
      totalTasks: tasks.length,
      onlineWorkers,
      totalCapacity,
      usedCapacity,
      recentTasks: tasks.slice(0, 10),
    }
  }, [tasks, workers])
}
```

**Step 5: Commit**

```bash
git commit -m "feat(dashboard-web): add TanStack Query hooks for tasks/workers/events"
```

---

## Phase 4: Pages

### Task 10: Overview Page

Dashboard overview with stats cards, worker summary, recent tasks.

**Files:**
- Modify: `packages/dashboard-web/src/pages/overview.tsx`
- Create: `packages/dashboard-web/src/components/overview/status-cards.tsx`
- Create: `packages/dashboard-web/src/components/overview/worker-summary.tsx`
- Create: `packages/dashboard-web/src/components/overview/recent-tasks.tsx`

**Step 1: Status cards component**

```tsx
// src/components/overview/status-cards.tsx
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'

const statusColors: Record<string, string> = {
  pending: 'text-yellow-600',
  assigned: 'text-blue-400',
  running: 'text-blue-600',
  paused: 'text-orange-500',
  blocked: 'text-red-400',
  completed: 'text-green-600',
  failed: 'text-red-600',
  timeout: 'text-orange-600',
  cancelled: 'text-gray-500',
}

export function StatusCards({ counts }: { counts: Record<string, number> }) {
  const statuses = ['pending', 'running', 'completed', 'failed', 'timeout', 'cancelled']

  return (
    <div className="grid grid-cols-2 gap-4 md:grid-cols-3 lg:grid-cols-6">
      {statuses.map((status) => (
        <Card key={status}>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium capitalize text-muted-foreground">
              {status}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className={`text-2xl font-bold ${statusColors[status]}`}>
              {counts[status] ?? 0}
            </div>
          </CardContent>
        </Card>
      ))}
    </div>
  )
}
```

**Step 2: Worker summary component**

```tsx
// src/components/overview/worker-summary.tsx
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'

interface WorkerSummaryProps {
  onlineWorkers: number
  totalCapacity: number
  usedCapacity: number
}

export function WorkerSummary({ onlineWorkers, totalCapacity, usedCapacity }: WorkerSummaryProps) {
  const utilization = totalCapacity > 0 ? Math.round((usedCapacity / totalCapacity) * 100) : 0

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm font-medium">Workers</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="flex justify-between text-sm">
          <span>{onlineWorkers} online</span>
          <span>{usedCapacity} / {totalCapacity} slots ({utilization}%)</span>
        </div>
        <Progress value={utilization} />
      </CardContent>
    </Card>
  )
}
```

**Step 3: Recent tasks table**

```tsx
// src/components/overview/recent-tasks.tsx
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import type { Task } from '@taskcast/core'
import { formatRelativeTime } from '@/lib/utils'

export function RecentTasks({ tasks }: { tasks: Task[] }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm font-medium">Recent Tasks</CardTitle>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>ID</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Created</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {tasks.map((task) => (
              <TableRow key={task.id}>
                <TableCell className="font-mono text-xs">{task.id.slice(-8)}</TableCell>
                <TableCell>{task.type ?? '—'}</TableCell>
                <TableCell>
                  <Badge variant={task.status === 'running' ? 'default' : 'secondary'}>
                    {task.status}
                  </Badge>
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {formatRelativeTime(task.createdAt)}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  )
}
```

**Step 4: Wire up overview page**

```tsx
// src/pages/overview.tsx
import { useStats } from '@/hooks/use-stats'
import { StatusCards } from '@/components/overview/status-cards'
import { WorkerSummary } from '@/components/overview/worker-summary'
import { RecentTasks } from '@/components/overview/recent-tasks'

export function OverviewPage() {
  const stats = useStats()

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-bold">Overview</h2>
      <StatusCards counts={stats.statusCounts} />
      <WorkerSummary
        onlineWorkers={stats.onlineWorkers}
        totalCapacity={stats.totalCapacity}
        usedCapacity={stats.usedCapacity}
      />
      <RecentTasks tasks={stats.recentTasks} />
    </div>
  )
}
```

**Step 5: Add formatRelativeTime to utils**

```typescript
// In src/lib/utils.ts, add:
export function formatRelativeTime(timestamp: number): string {
  const diff = Date.now() - timestamp
  if (diff < 60_000) return `${Math.round(diff / 1000)}s ago`
  if (diff < 3_600_000) return `${Math.round(diff / 60_000)}m ago`
  if (diff < 86_400_000) return `${Math.round(diff / 3_600_000)}h ago`
  return new Date(timestamp).toLocaleDateString()
}
```

**Step 6: Verify overview page renders, commit**

```bash
git commit -m "feat(dashboard-web): implement overview page with status cards + worker summary"
```

---

### Task 11: Tasks Page — List + Detail + Create

**Files:**
- Modify: `packages/dashboard-web/src/pages/tasks.tsx`
- Create: `packages/dashboard-web/src/components/tasks/task-table.tsx`
- Create: `packages/dashboard-web/src/components/tasks/task-detail.tsx`
- Create: `packages/dashboard-web/src/components/tasks/task-filters.tsx`
- Create: `packages/dashboard-web/src/components/tasks/create-task-dialog.tsx`
- Create: `packages/dashboard-web/src/components/tasks/task-actions.tsx`
- Create: `packages/dashboard-web/src/components/tasks/event-timeline.tsx`

**Step 1: Task filters bar**

```tsx
// src/components/tasks/task-filters.tsx
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Button } from '@/components/ui/button'

interface TaskFiltersProps {
  status: string
  type: string
  onStatusChange: (status: string) => void
  onTypeChange: (type: string) => void
  onCreateClick: () => void
}

const statuses = ['all', 'pending', 'assigned', 'running', 'paused', 'blocked', 'completed', 'failed', 'timeout', 'cancelled']

export function TaskFilters({ status, type, onStatusChange, onTypeChange, onCreateClick }: TaskFiltersProps) {
  return (
    <div className="flex items-center gap-3">
      <Select value={status} onValueChange={onStatusChange}>
        <SelectTrigger className="w-36">
          <SelectValue placeholder="Status" />
        </SelectTrigger>
        <SelectContent>
          {statuses.map((s) => (
            <SelectItem key={s} value={s}>{s === 'all' ? 'All statuses' : s}</SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Input
        placeholder="Filter by type..."
        value={type}
        onChange={(e) => onTypeChange(e.target.value)}
        className="w-48"
      />
      <div className="flex-1" />
      <Button onClick={onCreateClick}>Create Task</Button>
    </div>
  )
}
```

**Step 2: Task table**

```tsx
// src/components/tasks/task-table.tsx
import { Badge } from '@/components/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import type { Task } from '@taskcast/core'
import { formatRelativeTime } from '@/lib/utils'

interface TaskTableProps {
  tasks: Task[]
  selectedTaskId: string | null
  onSelect: (taskId: string) => void
}

export function TaskTable({ tasks, selectedTaskId, onSelect }: TaskTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>ID</TableHead>
          <TableHead>Type</TableHead>
          <TableHead>Status</TableHead>
          <TableHead>Hot</TableHead>
          <TableHead>Subs</TableHead>
          <TableHead>Worker</TableHead>
          <TableHead>Created</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {tasks.map((task: any) => (
          <TableRow
            key={task.id}
            className={`cursor-pointer ${selectedTaskId === task.id ? 'bg-muted' : ''}`}
            onClick={() => onSelect(task.id)}
          >
            <TableCell className="font-mono text-xs">{task.id.slice(-8)}</TableCell>
            <TableCell>{task.type ?? '—'}</TableCell>
            <TableCell>
              <Badge variant={task.status === 'failed' ? 'destructive' : 'secondary'}>
                {task.status}
              </Badge>
            </TableCell>
            <TableCell>
              <span className={task.hot ? 'text-orange-500' : 'text-blue-400'}>
                {task.hot ? '● hot' : '○ cold'}
              </span>
            </TableCell>
            <TableCell>{task.subscriberCount ?? 0}</TableCell>
            <TableCell className="font-mono text-xs">{task.assignedWorker?.slice(-6) ?? '—'}</TableCell>
            <TableCell className="text-muted-foreground">{formatRelativeTime(task.createdAt)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
```

**Step 3: Task detail panel**

```tsx
// src/components/tasks/task-detail.tsx
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Separator } from '@/components/ui/separator'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useTaskQuery } from '@/hooks/use-tasks'
import { useEventStream } from '@/hooks/use-events'
import { TaskActions } from './task-actions'
import { EventTimeline } from './event-timeline'

export function TaskDetail({ taskId, onClose }: { taskId: string; onClose: () => void }) {
  const { data: task, isLoading } = useTaskQuery(taskId)
  const { events, isDone } = useEventStream(taskId)

  if (isLoading || !task) return <div className="p-4">Loading...</div>

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between p-4">
        <div>
          <h3 className="font-mono text-lg">{task.id}</h3>
          <div className="flex items-center gap-2 mt-1">
            <Badge>{task.status}</Badge>
            <span className={(task as any).hot ? 'text-orange-500 text-sm' : 'text-blue-400 text-sm'}>
              {(task as any).hot ? '● hot' : '○ cold'}
            </span>
            <span className="text-sm text-muted-foreground">
              {(task as any).subscriberCount ?? 0} subscribers
            </span>
          </div>
        </div>
        <button onClick={onClose} className="text-muted-foreground hover:text-foreground">✕</button>
      </div>

      <Separator />

      <ScrollArea className="flex-1">
        <div className="space-y-4 p-4">
          {/* Task Info */}
          <Card>
            <CardHeader><CardTitle className="text-sm">Info</CardTitle></CardHeader>
            <CardContent className="space-y-2 text-sm">
              <div className="flex justify-between"><span>Type</span><span>{task.type ?? '—'}</span></div>
              <div className="flex justify-between"><span>TTL</span><span>{task.ttl ? `${task.ttl}s` : '—'}</span></div>
              <div className="flex justify-between"><span>Worker</span><span className="font-mono">{task.assignedWorker ?? '—'}</span></div>
              <div className="flex justify-between"><span>Created</span><span>{new Date(task.createdAt).toLocaleString()}</span></div>
              {task.completedAt && (
                <div className="flex justify-between"><span>Completed</span><span>{new Date(task.completedAt).toLocaleString()}</span></div>
              )}
            </CardContent>
          </Card>

          {/* Params */}
          {task.params && (
            <Card>
              <CardHeader><CardTitle className="text-sm">Params</CardTitle></CardHeader>
              <CardContent>
                <pre className="rounded bg-muted p-3 text-xs overflow-auto">{JSON.stringify(task.params, null, 2)}</pre>
              </CardContent>
            </Card>
          )}

          {/* Result / Error */}
          {task.result && (
            <Card>
              <CardHeader><CardTitle className="text-sm">Result</CardTitle></CardHeader>
              <CardContent>
                <pre className="rounded bg-muted p-3 text-xs overflow-auto">{JSON.stringify(task.result, null, 2)}</pre>
              </CardContent>
            </Card>
          )}
          {task.error && (
            <Card>
              <CardHeader><CardTitle className="text-sm text-destructive">Error</CardTitle></CardHeader>
              <CardContent>
                <pre className="rounded bg-muted p-3 text-xs overflow-auto">{JSON.stringify(task.error, null, 2)}</pre>
              </CardContent>
            </Card>
          )}

          {/* Actions */}
          <TaskActions taskId={taskId} currentStatus={task.status} />

          {/* Event Timeline */}
          <Card>
            <CardHeader>
              <CardTitle className="text-sm">
                Events {isDone ? '(stream ended)' : '(live)'}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <EventTimeline events={events} />
            </CardContent>
          </Card>
        </div>
      </ScrollArea>
    </div>
  )
}
```

**Step 4: Task actions (status transitions)**

```tsx
// src/components/tasks/task-actions.tsx
import { Button } from '@/components/ui/button'
import { useTransitionTask } from '@/hooks/use-tasks'
import type { TaskStatus } from '@taskcast/core'

const validTransitions: Record<string, TaskStatus[]> = {
  pending: ['running', 'cancelled'],
  assigned: ['running', 'cancelled'],
  running: ['completed', 'failed', 'cancelled', 'paused'],
  paused: ['running', 'cancelled'],
  blocked: ['running', 'cancelled'],
}

export function TaskActions({ taskId, currentStatus }: { taskId: string; currentStatus: TaskStatus }) {
  const transition = useTransitionTask()
  const available = validTransitions[currentStatus] ?? []

  if (available.length === 0) return null

  return (
    <Card>
      <CardHeader><CardTitle className="text-sm">Actions</CardTitle></CardHeader>
      <CardContent className="flex flex-wrap gap-2">
        {available.map((status) => (
          <Button
            key={status}
            variant={status === 'cancelled' || status === 'failed' ? 'destructive' : 'outline'}
            size="sm"
            onClick={() => transition.mutate({ taskId, status })}
            disabled={transition.isPending}
          >
            {status}
          </Button>
        ))}
      </CardContent>
    </Card>
  )
}
```

(Note: `TaskActions` needs `Card` imports added)

**Step 5: Event timeline**

```tsx
// src/components/tasks/event-timeline.tsx
import { Badge } from '@/components/ui/badge'
import type { SSEEnvelope } from '@taskcast/core'

const levelColors: Record<string, string> = {
  debug: 'bg-gray-100 text-gray-700',
  info: 'bg-blue-100 text-blue-700',
  warn: 'bg-yellow-100 text-yellow-700',
  error: 'bg-red-100 text-red-700',
}

export function EventTimeline({ events }: { events: SSEEnvelope[] }) {
  if (events.length === 0) return <p className="text-sm text-muted-foreground">No events yet</p>

  return (
    <div className="space-y-2 max-h-96 overflow-auto">
      {events.map((event, i) => (
        <div key={event.eventId ?? i} className="flex items-start gap-3 rounded border p-2 text-sm">
          <Badge className={levelColors[event.level] ?? ''} variant="outline">
            {event.level}
          </Badge>
          <div className="flex-1 min-w-0">
            <div className="flex items-center justify-between">
              <span className="font-mono text-xs font-medium">{event.type}</span>
              <span className="text-xs text-muted-foreground">
                #{event.rawIndex}
              </span>
            </div>
            <pre className="mt-1 text-xs text-muted-foreground overflow-auto">
              {JSON.stringify(event.data, null, 2)}
            </pre>
          </div>
        </div>
      ))}
    </div>
  )
}
```

**Step 6: Create task dialog**

```tsx
// src/components/tasks/create-task-dialog.tsx
import { useState } from 'react'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useCreateTask } from '@/hooks/use-tasks'

export function CreateTaskDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [type, setType] = useState('')
  const [params, setParams] = useState('{}')
  const [ttl, setTtl] = useState('')
  const [tags, setTags] = useState('')
  const createTask = useCreateTask()

  const handleCreate = async () => {
    try {
      const parsedParams = JSON.parse(params)
      await createTask.mutateAsync({
        type: type || undefined,
        params: parsedParams,
        ttl: ttl ? Number(ttl) : undefined,
        tags: tags ? tags.split(',').map((t) => t.trim()) : undefined,
      })
      onClose()
      setType('')
      setParams('{}')
      setTtl('')
      setTags('')
    } catch (err) {
      // TODO: show error
    }
  }

  return (
    <Dialog open={open} onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader><DialogTitle>Create Task</DialogTitle></DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium">Type</label>
            <Input value={type} onChange={(e) => setType(e.target.value)} placeholder="llm.chat" />
          </div>
          <div className="space-y-2">
            <label className="text-sm font-medium">Params (JSON)</label>
            <textarea
              className="w-full rounded border p-2 font-mono text-sm"
              rows={4}
              value={params}
              onChange={(e) => setParams(e.target.value)}
            />
          </div>
          <div className="flex gap-3">
            <div className="flex-1 space-y-2">
              <label className="text-sm font-medium">TTL (seconds)</label>
              <Input value={ttl} onChange={(e) => setTtl(e.target.value)} placeholder="300" type="number" />
            </div>
            <div className="flex-1 space-y-2">
              <label className="text-sm font-medium">Tags (comma-separated)</label>
              <Input value={tags} onChange={(e) => setTags(e.target.value)} placeholder="batch,priority" />
            </div>
          </div>
          <Button onClick={handleCreate} disabled={createTask.isPending} className="w-full">
            {createTask.isPending ? 'Creating...' : 'Create Task'}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
```

**Step 7: Wire up tasks page**

```tsx
// src/pages/tasks.tsx
import { useState } from 'react'
import { useParams } from 'react-router-dom'
import { useTasksQuery } from '@/hooks/use-tasks'
import { TaskTable } from '@/components/tasks/task-table'
import { TaskDetail } from '@/components/tasks/task-detail'
import { TaskFilters } from '@/components/tasks/task-filters'
import { CreateTaskDialog } from '@/components/tasks/create-task-dialog'

export function TasksPage() {
  const { taskId: urlTaskId } = useParams()
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(urlTaskId ?? null)
  const [statusFilter, setStatusFilter] = useState('all')
  const [typeFilter, setTypeFilter] = useState('')
  const [createOpen, setCreateOpen] = useState(false)

  const filter = {
    ...(statusFilter !== 'all' ? { status: statusFilter } : {}),
    ...(typeFilter ? { type: typeFilter } : {}),
  }
  const { data: tasks = [], isLoading } = useTasksQuery(filter)

  return (
    <div className="flex h-full gap-4">
      {/* Left: Task List */}
      <div className="flex flex-1 flex-col space-y-4">
        <h2 className="text-2xl font-bold">Tasks</h2>
        <TaskFilters
          status={statusFilter}
          type={typeFilter}
          onStatusChange={setStatusFilter}
          onTypeChange={setTypeFilter}
          onCreateClick={() => setCreateOpen(true)}
        />
        {isLoading ? (
          <div>Loading...</div>
        ) : (
          <TaskTable
            tasks={tasks}
            selectedTaskId={selectedTaskId}
            onSelect={setSelectedTaskId}
          />
        )}
      </div>

      {/* Right: Detail Panel */}
      {selectedTaskId && (
        <div className="w-96 border-l">
          <TaskDetail taskId={selectedTaskId} onClose={() => setSelectedTaskId(null)} />
        </div>
      )}

      <CreateTaskDialog open={createOpen} onClose={() => setCreateOpen(false)} />
    </div>
  )
}
```

**Step 8: Verify tasks page renders, commit**

```bash
git commit -m "feat(dashboard-web): implement tasks page with list, detail, create dialog"
```

---

### Task 12: Events Page

Dedicated real-time event stream viewer.

**Files:**
- Modify: `packages/dashboard-web/src/pages/events.tsx`
- Create: `packages/dashboard-web/src/components/events/event-filters.tsx`
- Create: `packages/dashboard-web/src/components/events/live-stream.tsx`

**Step 1: Events page with task selector + filter + live stream**

```tsx
// src/pages/events.tsx
import { useState } from 'react'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { useEventStream } from '@/hooks/use-events'
import { EventTimeline } from '@/components/tasks/event-timeline'

const levels = ['debug', 'info', 'warn', 'error'] as const

export function EventsPage() {
  const [taskId, setTaskId] = useState('')
  const [subscribedTaskId, setSubscribedTaskId] = useState<string | null>(null)
  const [typeFilter, setTypeFilter] = useState('')
  const [levelFilter, setLevelFilter] = useState<string[]>(['info', 'warn', 'error'])

  const filter = {
    types: typeFilter || undefined,
    levels: levelFilter.join(',') || undefined,
  }
  const { events, isDone, doneReason, error } = useEventStream(subscribedTaskId, filter)

  const toggleLevel = (level: string) => {
    setLevelFilter((prev) =>
      prev.includes(level) ? prev.filter((l) => l !== level) : [...prev, level],
    )
  }

  return (
    <div className="space-y-4">
      <h2 className="text-2xl font-bold">Event Stream</h2>

      {/* Controls */}
      <div className="flex items-center gap-3">
        <Input
          placeholder="Task ID"
          value={taskId}
          onChange={(e) => setTaskId(e.target.value)}
          className="w-64"
        />
        <Button
          onClick={() => setSubscribedTaskId(taskId || null)}
          variant={subscribedTaskId ? 'destructive' : 'default'}
        >
          {subscribedTaskId ? 'Unsubscribe' : 'Subscribe'}
        </Button>
        <Input
          placeholder="Type filter (e.g. llm.*)"
          value={typeFilter}
          onChange={(e) => setTypeFilter(e.target.value)}
          className="w-48"
        />
        <div className="flex gap-1">
          {levels.map((level) => (
            <Badge
              key={level}
              variant={levelFilter.includes(level) ? 'default' : 'outline'}
              className="cursor-pointer"
              onClick={() => toggleLevel(level)}
            >
              {level}
            </Badge>
          ))}
        </div>
      </div>

      {/* Status */}
      {subscribedTaskId && (
        <div className="flex items-center gap-2 text-sm">
          <span className={isDone ? 'text-muted-foreground' : 'text-green-600'}>
            {isDone ? `Stream ended: ${doneReason}` : '● Streaming live'}
          </span>
          <span className="text-muted-foreground">{events.length} events</span>
          {error && <span className="text-destructive">{error.message}</span>}
        </div>
      )}

      {/* Event Stream */}
      {subscribedTaskId ? (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Events for {subscribedTaskId}</CardTitle>
          </CardHeader>
          <CardContent>
            <EventTimeline events={events} />
          </CardContent>
        </Card>
      ) : (
        <p className="text-muted-foreground">Enter a Task ID and click Subscribe to start streaming events.</p>
      )}
    </div>
  )
}
```

**Step 2: Commit**

```bash
git commit -m "feat(dashboard-web): implement events page with live SSE stream"
```

---

### Task 13: Workers Page

Worker list with capacity visualization, drain/disconnect controls, and optional resource data.

**Files:**
- Modify: `packages/dashboard-web/src/pages/workers.tsx`
- Create: `packages/dashboard-web/src/components/workers/worker-table.tsx`
- Create: `packages/dashboard-web/src/components/workers/worker-detail.tsx`

**Step 1: Worker table**

```tsx
// src/components/workers/worker-table.tsx
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useDrainWorker, useDisconnectWorker } from '@/hooks/use-workers'
import { formatRelativeTime } from '@/lib/utils'
import type { Worker } from '@taskcast/core'

const statusBadgeVariant: Record<string, 'default' | 'secondary' | 'destructive' | 'outline'> = {
  idle: 'default',
  busy: 'secondary',
  draining: 'outline',
  offline: 'destructive',
}

export function WorkerTable({ workers, onSelect }: { workers: Worker[]; onSelect: (id: string) => void }) {
  const drain = useDrainWorker()
  const disconnect = useDisconnectWorker()

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>ID</TableHead>
          <TableHead>Status</TableHead>
          <TableHead>Capacity</TableHead>
          <TableHead>Mode</TableHead>
          <TableHead>Weight</TableHead>
          <TableHead>Last Heartbeat</TableHead>
          <TableHead>Actions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {workers.map((worker) => {
          const utilization = worker.capacity > 0 ? Math.round((worker.usedSlots / worker.capacity) * 100) : 0
          return (
            <TableRow key={worker.id} className="cursor-pointer" onClick={() => onSelect(worker.id)}>
              <TableCell className="font-mono text-xs">{worker.id.slice(-8)}</TableCell>
              <TableCell>
                <Badge variant={statusBadgeVariant[worker.status] ?? 'secondary'}>
                  {worker.status}
                </Badge>
              </TableCell>
              <TableCell>
                <div className="flex items-center gap-2 w-32">
                  <Progress value={utilization} className="h-2" />
                  <span className="text-xs text-muted-foreground">
                    {worker.usedSlots}/{worker.capacity}
                  </span>
                </div>
              </TableCell>
              <TableCell>{worker.connectionMode}</TableCell>
              <TableCell>{worker.weight}</TableCell>
              <TableCell className="text-muted-foreground">
                {formatRelativeTime(worker.lastHeartbeatAt)}
              </TableCell>
              <TableCell>
                <div className="flex gap-1" onClick={(e) => e.stopPropagation()}>
                  {worker.status !== 'draining' && worker.status !== 'offline' && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => drain.mutate({ workerId: worker.id, status: 'draining' })}
                      disabled={drain.isPending}
                    >
                      Drain
                    </Button>
                  )}
                  {worker.status === 'draining' && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => drain.mutate({ workerId: worker.id, status: 'idle' })}
                      disabled={drain.isPending}
                    >
                      Resume
                    </Button>
                  )}
                  <Button
                    variant="destructive"
                    size="sm"
                    onClick={() => {
                      if (confirm('Force disconnect this worker?')) {
                        disconnect.mutate(worker.id)
                      }
                    }}
                    disabled={disconnect.isPending}
                  >
                    Disconnect
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}
```

**Step 2: Worker detail (expandable)**

```tsx
// src/components/workers/worker-detail.tsx
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import type { Worker } from '@taskcast/core'

export function WorkerDetail({ worker }: { worker: Worker }) {
  return (
    <div className="space-y-4 p-4">
      <Card>
        <CardHeader><CardTitle className="text-sm">Connection Info</CardTitle></CardHeader>
        <CardContent className="space-y-2 text-sm">
          <div className="flex justify-between"><span>ID</span><span className="font-mono">{worker.id}</span></div>
          <div className="flex justify-between"><span>Mode</span><span>{worker.connectionMode}</span></div>
          <div className="flex justify-between"><span>Weight</span><span>{worker.weight}</span></div>
          <div className="flex justify-between"><span>Connected</span><span>{new Date(worker.connectedAt).toLocaleString()}</span></div>
        </CardContent>
      </Card>

      {worker.matchRule && (
        <Card>
          <CardHeader><CardTitle className="text-sm">Match Rule</CardTitle></CardHeader>
          <CardContent>
            <pre className="rounded bg-muted p-3 text-xs">{JSON.stringify(worker.matchRule, null, 2)}</pre>
          </CardContent>
        </Card>
      )}

      {/* Optional resource data from metadata */}
      {worker.metadata && Object.keys(worker.metadata).length > 0 && (
        <Card>
          <CardHeader><CardTitle className="text-sm">Resource Data</CardTitle></CardHeader>
          <CardContent>
            <div className="space-y-1 text-sm">
              {Object.entries(worker.metadata).map(([key, value]) => (
                <div key={key} className="flex justify-between">
                  <span className="text-muted-foreground">{key}</span>
                  <span className="font-mono">{typeof value === 'object' ? JSON.stringify(value) : String(value)}</span>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
```

**Step 3: Workers page**

```tsx
// src/pages/workers.tsx
import { useState } from 'react'
import { useWorkersQuery } from '@/hooks/use-workers'
import { WorkerTable } from '@/components/workers/worker-table'
import { WorkerDetail } from '@/components/workers/worker-detail'

export function WorkersPage() {
  const { data: workers = [], isLoading } = useWorkersQuery()
  const [selectedWorkerId, setSelectedWorkerId] = useState<string | null>(null)
  const selectedWorker = workers.find((w: any) => w.id === selectedWorkerId)

  return (
    <div className="flex h-full gap-4">
      <div className="flex flex-1 flex-col space-y-4">
        <h2 className="text-2xl font-bold">Workers</h2>
        <div className="text-sm text-muted-foreground">
          {workers.length} workers registered
        </div>
        {isLoading ? (
          <div>Loading...</div>
        ) : workers.length === 0 ? (
          <p className="text-muted-foreground">No workers connected.</p>
        ) : (
          <WorkerTable workers={workers} onSelect={setSelectedWorkerId} />
        )}
      </div>

      {selectedWorker && (
        <div className="w-80 border-l">
          <WorkerDetail worker={selectedWorker} />
        </div>
      )}
    </div>
  )
}
```

**Step 4: Commit**

```bash
git commit -m "feat(dashboard-web): implement workers page with capacity bars + drain/disconnect"
```

---

## Phase 5: Integration

### Task 14: CLI `taskcast ui` Subcommand

Add `ui` subcommand to CLI that serves the dashboard's built static files.

**Files:**
- Modify: `packages/cli/src/index.ts` — add `ui` command
- Modify: `packages/cli/package.json` — add `@taskcast/dashboard-web` dependency
- Create: `packages/dashboard-web/src/dist-path.ts` — export dist directory path

**Step 1: Export dist path from dashboard-web**

```typescript
// packages/dashboard-web/src/dist-path.ts
import { fileURLToPath } from 'url'
import { dirname, join } from 'path'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

export const dashboardDistPath = join(__dirname, '..', 'dist')
```

Add to `packages/dashboard-web/package.json` exports:
```json
"exports": {
  "./dist-path": {
    "import": "./src/dist-path.ts"
  }
}
```

**Step 2: Add ui command to CLI**

In `packages/cli/src/index.ts`, add a new command:

```typescript
import { serve } from '@hono/node-server'
import { serveStatic } from '@hono/node-server/serve-static'
import { Hono } from 'hono'

program
  .command('ui')
  .description('Start the Taskcast Dashboard web UI')
  .option('-p, --port <port>', 'Dashboard port', '3722')
  .option('-s, --server <url>', 'Taskcast server URL', 'http://localhost:3721')
  .option('--admin-token <token>', 'Admin token for auto-connect')
  .action(async (opts) => {
    const { dashboardDistPath } = await import('@taskcast/dashboard-web/dist-path')
    const dashboardApp = new Hono()

    // Auto-connect config endpoint
    dashboardApp.get('/api/config', (c) => {
      return c.json({
        baseUrl: opts.server,
        adminToken: opts.adminToken,
      })
    })

    // Serve static dashboard files
    dashboardApp.use('/*', serveStatic({ root: dashboardDistPath }))

    // SPA fallback
    dashboardApp.get('*', serveStatic({ root: dashboardDistPath, path: 'index.html' }))

    const port = Number(opts.port)
    serve({ fetch: dashboardApp.fetch, port }, () => {
      console.log(`Taskcast Dashboard running at http://localhost:${port}`)
      console.log(`Connected to Taskcast server: ${opts.server}`)
    })
  })
```

**Step 3: Add dependency**

In `packages/cli/package.json`:
```json
"dependencies": {
  "@taskcast/dashboard-web": "workspace:*"
}
```

**Step 4: Build dashboard and test CLI serve**

```bash
cd packages/dashboard-web && pnpm build
cd packages/cli && npx tsx src/index.ts ui --server http://localhost:3721
```

Expected: Dashboard available at http://localhost:3722

**Step 5: Commit**

```bash
git commit -m "feat(cli): add 'taskcast ui' command to serve dashboard"
```

---

### Task 15: Docker Support

Create Dockerfile for standalone dashboard deployment.

**Files:**
- Create: `packages/dashboard-web/Dockerfile`
- Create: `packages/dashboard-web/nginx.conf`

**Step 1: Create nginx config**

```nginx
# packages/dashboard-web/nginx.conf
server {
    listen 80;
    root /usr/share/nginx/html;
    index index.html;

    location / {
        try_files $uri $uri/ /index.html;
    }

    location /api/config {
        default_type application/json;
        return 200 '{"baseUrl":"${TASKCAST_SERVER_URL}"}';
    }
}
```

**Step 2: Create Dockerfile**

```dockerfile
# packages/dashboard-web/Dockerfile
FROM node:20-alpine AS builder
WORKDIR /app
COPY . .
RUN npm install -g pnpm && pnpm install && pnpm --filter @taskcast/dashboard-web build

FROM nginx:alpine
COPY --from=builder /app/packages/dashboard-web/dist /usr/share/nginx/html
COPY packages/dashboard-web/nginx.conf /etc/nginx/conf.d/default.conf
ENV TASKCAST_SERVER_URL=http://localhost:3721
EXPOSE 80
```

**Step 3: Test Docker build**

```bash
docker build -f packages/dashboard-web/Dockerfile -t taskcast-dashboard .
docker run -p 8080:80 -e TASKCAST_SERVER_URL=http://host.docker.internal:3721 taskcast-dashboard
```

**Step 4: Commit**

```bash
git commit -m "feat(dashboard-web): add Docker support with nginx"
```

---

### Task 16: Final Verification

**Step 1: Build all packages**

```bash
pnpm build
```

**Step 2: Run all tests**

```bash
pnpm test
```

**Step 3: Verify dashboard end-to-end**

1. Start a Taskcast server: `cd packages/cli && npx tsx src/index.ts`
2. In another terminal: `cd packages/cli && npx tsx src/index.ts ui`
3. Open http://localhost:3722
4. Login with admin token (printed in server terminal)
5. Create a task, verify it appears in the list
6. Check overview stats
7. Subscribe to events on the events page
8. View workers page

**Step 4: Final commit**

```bash
git commit -m "feat(dashboard-web): complete v1 dashboard implementation"
```
