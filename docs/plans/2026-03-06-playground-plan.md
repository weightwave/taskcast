# @taskcast/playground Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build an interactive multi-role playground for Taskcast — simulate backends, browsers, and workers in a single web UI for debugging and demos.

**Architecture:** React SPA (Vite + shadcn/ui + Zustand) with an embedded Taskcast dev server. Multi-panel layout where each panel represents a role instance (Backend REST, Browser SSE, Worker Pull, Worker WS). All panels interact through the real Taskcast service, authentically simulating distributed scenarios.

**Tech Stack:** React 18, Vite, shadcn/ui (Tailwind v4), Zustand, @taskcast/core + @taskcast/server + @taskcast/client

**Design Doc:** `docs/plans/2026-03-06-playground-design.md`

---

### Task 1: Package Scaffolding

Create the package directory structure, package.json, and all config files.

**Files:**
- Create: `packages/playground/package.json`
- Create: `packages/playground/tsconfig.json`
- Create: `packages/playground/tsconfig.node.json`
- Create: `packages/playground/vite.config.ts`
- Create: `packages/playground/index.html`
- Create: `packages/playground/src/main.tsx`
- Create: `packages/playground/src/App.tsx`
- Create: `packages/playground/src/index.css`
- Modify: `tsconfig.json` (root — add project reference)

**Step 1: Create package.json**

```json
{
  "name": "@taskcast/playground",
  "version": "0.3.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "tsx dev-server/server.ts",
    "build": "vite build",
    "preview": "vite preview"
  },
  "dependencies": {
    "@taskcast/client": "workspace:*",
    "@taskcast/core": "workspace:*",
    "@taskcast/react": "workspace:*",
    "@taskcast/server": "workspace:*",
    "hono": "^4.7.4",
    "@hono/node-server": "^1.13.7",
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "zustand": "^5.0.3"
  },
  "devDependencies": {
    "@types/node": "^22.10.0",
    "@types/react": "^18.3.14",
    "@types/react-dom": "^18.3.2",
    "@vitejs/plugin-react": "^4.3.4",
    "@tailwindcss/vite": "^4.0.0",
    "tailwindcss": "^4.0.0",
    "tsx": "^4.19.0",
    "vite": "^6.0.0"
  }
}
```

**Step 2: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noEmit": true,
    "isolatedModules": true,
    "skipLibCheck": true,
    "baseUrl": ".",
    "paths": {
      "@/*": ["./src/*"]
    }
  },
  "include": ["src"]
}
```

**Step 3: Create tsconfig.node.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true,
    "isolatedModules": true,
    "skipLibCheck": true
  },
  "include": ["vite.config.ts", "dev-server"]
}
```

**Step 4: Create vite.config.ts**

```typescript
import path from 'path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    proxy: {
      '/taskcast': {
        target: 'http://localhost:3721',
        changeOrigin: true,
      },
      '/workers': {
        target: 'http://localhost:3721',
        changeOrigin: true,
        ws: true,
      },
    },
  },
})
```

**Step 5: Create index.html**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Taskcast Playground</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

**Step 6: Create src/main.tsx**

```tsx
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App.tsx'
import './index.css'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
```

**Step 7: Create src/App.tsx (placeholder)**

```tsx
export function App() {
  return <div className="min-h-screen bg-background text-foreground p-4">Taskcast Playground</div>
}
```

**Step 8: Create src/index.css (placeholder — will be replaced by shadcn init)**

```css
@import "tailwindcss";
```

**Step 9: Add project reference to root tsconfig.json**

Add `{ "path": "packages/playground" }` to the `references` array in the root `tsconfig.json`. Note: since this package uses `noEmit: true` (no build output — Vite handles bundling), the root `pnpm build` (which runs `tsc -b`) will just type-check it.

**Step 10: Install dependencies**

```bash
cd packages/playground && pnpm install
```

**Step 11: Initialize shadcn/ui**

```bash
cd packages/playground && pnpm dlx shadcn@latest init
```

Select: New York style, Neutral base color. This creates `components.json`, updates `src/index.css` with CSS variables, creates `src/lib/utils.ts`.

**Step 12: Add shadcn components**

```bash
cd packages/playground && pnpm dlx shadcn@latest add resizable tabs button input select card badge scroll-area dialog dropdown-menu separator textarea label tooltip
```

**Step 13: Verify it runs**

```bash
cd packages/playground && pnpm dlx vite
```

Open http://localhost:5173 — should show "Taskcast Playground" with correct styling.

**Step 14: Commit**

```bash
git add packages/playground
git commit -m "feat(playground): scaffold package with Vite + React + shadcn/ui"
```

---

### Task 2: Dev Server (Embedded Taskcast)

Create the embedded Taskcast HTTP server that runs alongside Vite.

**Files:**
- Create: `packages/playground/dev-server/server.ts`

**Step 1: Create dev-server/server.ts**

```typescript
import { serve } from '@hono/node-server'
import { Hono } from 'hono'
import { cors } from 'hono/cors'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { WorkerManager } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createServer } from 'vite'

async function main() {
  // Create Taskcast engine with in-memory adapters
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore: new MemoryShortTermStore(),
  })

  const workerManager = new WorkerManager({ engine })

  // Create the Taskcast Hono app
  const taskcastApp = createTaskcastApp({ engine, workerManager })

  // Create a top-level Hono app
  const app = new Hono()
  app.use('*', cors())
  app.route('/taskcast', taskcastApp)

  // Mount the worker routes at top level too (for /workers/ws WebSocket path)
  // The createTaskcastApp already mounts workers under /taskcast/workers,
  // but we also want /workers for the Vite proxy convenience
  app.route('/workers', taskcastApp)

  // Start the Taskcast server
  const taskcastPort = 3721
  const server = serve({ fetch: app.fetch, port: taskcastPort }, (info) => {
    console.log(`Taskcast server running at http://localhost:${info.port}`)
  })

  // Handle WebSocket upgrade for worker WS connections
  // (hono/node-server needs explicit upgrade handling)

  // Start Vite dev server
  const vite = await createServer({
    configFile: new URL('../vite.config.ts', import.meta.url).pathname,
    root: new URL('..', import.meta.url).pathname,
    server: { port: 5173 },
  })
  await vite.listen()
  console.log(`Playground UI at http://localhost:5173`)

  // Graceful shutdown
  process.on('SIGINT', async () => {
    await vite.close()
    server.close()
    process.exit(0)
  })
}

main().catch(console.error)
```

**Step 2: Test the dev server**

```bash
cd packages/playground && pnpm dev
```

- Open http://localhost:5173 — should show the placeholder UI
- Open http://localhost:3721/taskcast/health — should return `{ "ok": true }`

**Step 3: Commit**

```bash
git add packages/playground/dev-server
git commit -m "feat(playground): add embedded Taskcast dev server"
```

---

### Task 3: Zustand Stores

Create the state management stores for connection, panels, and global data.

**Files:**
- Create: `packages/playground/src/stores/connection.ts`
- Create: `packages/playground/src/stores/panels.ts`
- Create: `packages/playground/src/stores/data.ts`
- Create: `packages/playground/src/stores/index.ts`

**Step 1: Create connection store**

```typescript
// packages/playground/src/stores/connection.ts
import { create } from 'zustand'

export interface ConnectionState {
  mode: 'embedded' | 'external'
  baseUrl: string
  token: string
  connected: boolean

  setMode: (mode: 'embedded' | 'external') => void
  setBaseUrl: (url: string) => void
  setToken: (token: string) => void
  setConnected: (connected: boolean) => void
}

export const useConnectionStore = create<ConnectionState>((set) => ({
  mode: 'embedded',
  baseUrl: '/taskcast',
  token: '',
  connected: false,

  setMode: (mode) =>
    set({
      mode,
      baseUrl: mode === 'embedded' ? '/taskcast' : '',
    }),
  setBaseUrl: (baseUrl) => set({ baseUrl }),
  setToken: (token) => set({ token }),
  setConnected: (connected) => set({ connected }),
}))
```

**Step 2: Create panels store**

```typescript
// packages/playground/src/stores/panels.ts
import { create } from 'zustand'

export type PanelType = 'backend' | 'browser' | 'worker-pull' | 'worker-ws'

export interface Panel {
  id: string
  type: PanelType
  label: string
  customToken?: string  // override global token
  useAuth: 'global' | 'custom' | 'none'
}

interface PanelState {
  panels: Panel[]
  addPanel: (type: PanelType) => void
  removePanel: (id: string) => void
  updatePanel: (id: string, update: Partial<Panel>) => void
}

let counter = 0
const labelMap: Record<PanelType, string> = {
  backend: 'Backend',
  browser: 'Browser',
  'worker-pull': 'Worker (Pull)',
  'worker-ws': 'Worker (WS)',
}

export const usePanelStore = create<PanelState>((set) => ({
  panels: [],

  addPanel: (type) =>
    set((state) => {
      counter++
      const typeCount = state.panels.filter((p) => p.type === type).length + 1
      return {
        panels: [
          ...state.panels,
          {
            id: `panel-${counter}`,
            type,
            label: `${labelMap[type]} ${typeCount}`,
            useAuth: 'global',
          },
        ],
      }
    }),

  removePanel: (id) =>
    set((state) => ({
      panels: state.panels.filter((p) => p.id !== id),
    })),

  updatePanel: (id, update) =>
    set((state) => ({
      panels: state.panels.map((p) => (p.id === id ? { ...p, ...update } : p)),
    })),
}))
```

**Step 3: Create data store**

```typescript
// packages/playground/src/stores/data.ts
import { create } from 'zustand'
import type { Task, TaskEvent } from '@taskcast/core'

export interface WebhookLog {
  id: string
  timestamp: number
  url: string
  payload: unknown
  statusCode?: number
  error?: string
}

interface DataState {
  tasks: Task[]
  globalEvents: TaskEvent[]
  webhookLogs: WebhookLog[]

  setTasks: (tasks: Task[]) => void
  addEvent: (event: TaskEvent) => void
  addWebhookLog: (log: WebhookLog) => void
  clearAll: () => void
}

export const useDataStore = create<DataState>((set) => ({
  tasks: [],
  globalEvents: [],
  webhookLogs: [],

  setTasks: (tasks) => set({ tasks }),
  addEvent: (event) =>
    set((state) => ({
      globalEvents: [event, ...state.globalEvents].slice(0, 500),
    })),
  addWebhookLog: (log) =>
    set((state) => ({
      webhookLogs: [log, ...state.webhookLogs].slice(0, 200),
    })),
  clearAll: () => set({ tasks: [], globalEvents: [], webhookLogs: [] }),
}))
```

**Step 4: Create barrel export**

```typescript
// packages/playground/src/stores/index.ts
export { useConnectionStore } from './connection.js'
export { usePanelStore } from './panels.js'
export type { PanelType, Panel } from './panels.js'
export { useDataStore } from './data.js'
export type { WebhookLog } from './data.js'
```

**Step 5: Verify no type errors**

```bash
cd packages/playground && npx tsc --noEmit
```

**Step 6: Commit**

```bash
git add packages/playground/src/stores
git commit -m "feat(playground): add Zustand stores for connection, panels, data"
```

---

### Task 4: Layout Shell

Build the overall layout: TopBar, PanelContainer, BottomArea.

**Files:**
- Create: `packages/playground/src/components/layout/TopBar.tsx`
- Create: `packages/playground/src/components/layout/PanelContainer.tsx`
- Create: `packages/playground/src/components/layout/BottomArea.tsx`
- Modify: `packages/playground/src/App.tsx`

**Step 1: Create TopBar**

TopBar contains:
- Server mode selector (embedded / external)
- Base URL display/input
- Global token input
- Connection status indicator
- "Add Role" dropdown

```tsx
// packages/playground/src/components/layout/TopBar.tsx
import { useConnectionStore, usePanelStore } from '@/stores'
import type { PanelType } from '@/stores'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Badge } from '@/components/ui/badge'

export function TopBar() {
  const { mode, setMode, baseUrl, setBaseUrl, token, setToken, connected } =
    useConnectionStore()
  const addPanel = usePanelStore((s) => s.addPanel)

  const panelOptions: { type: PanelType; label: string }[] = [
    { type: 'backend', label: 'Backend (REST)' },
    { type: 'browser', label: 'Browser (SSE)' },
    { type: 'worker-pull', label: 'Worker (Pull)' },
    { type: 'worker-ws', label: 'Worker (WS)' },
  ]

  return (
    <div className="flex items-center gap-3 p-3 border-b bg-card">
      <span className="font-semibold text-sm">Taskcast Playground</span>

      <Select value={mode} onValueChange={(v) => setMode(v as 'embedded' | 'external')}>
        <SelectTrigger className="w-[130px]">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="embedded">Embedded</SelectItem>
          <SelectItem value="external">External</SelectItem>
        </SelectContent>
      </Select>

      {mode === 'external' ? (
        <Input
          className="w-[280px]"
          placeholder="http://localhost:3721"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
        />
      ) : (
        <span className="text-xs text-muted-foreground">{baseUrl}</span>
      )}

      <Badge variant={connected ? 'default' : 'secondary'}>
        {connected ? 'Connected' : 'Disconnected'}
      </Badge>

      <div className="flex-1" />

      <Input
        className="w-[200px]"
        placeholder="JWT Token (optional)"
        type="password"
        value={token}
        onChange={(e) => setToken(e.target.value)}
      />

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm">
            + Add Role
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          {panelOptions.map((opt) => (
            <DropdownMenuItem key={opt.type} onClick={() => addPanel(opt.type)}>
              {opt.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}
```

**Step 2: Create PanelContainer**

Uses `ResizablePanelGroup` to display all active panels side by side.

```tsx
// packages/playground/src/components/layout/PanelContainer.tsx
import { usePanelStore } from '@/stores'
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@/components/ui/resizable'
import { BackendPanel } from '@/components/panels/BackendPanel'
import { BrowserPanel } from '@/components/panels/BrowserPanel'
import { WorkerPullPanel } from '@/components/panels/WorkerPullPanel'
import { WorkerWsPanel } from '@/components/panels/WorkerWsPanel'
import type { Panel } from '@/stores'

function PanelRenderer({ panel }: { panel: Panel }) {
  switch (panel.type) {
    case 'backend':
      return <BackendPanel panel={panel} />
    case 'browser':
      return <BrowserPanel panel={panel} />
    case 'worker-pull':
      return <WorkerPullPanel panel={panel} />
    case 'worker-ws':
      return <WorkerWsPanel panel={panel} />
  }
}

export function PanelContainer() {
  const panels = usePanelStore((s) => s.panels)

  if (panels.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground">
        Click "+ Add Role" to get started
      </div>
    )
  }

  return (
    <ResizablePanelGroup direction="horizontal" className="flex-1">
      {panels.map((panel, i) => (
        <div key={panel.id} className="contents">
          {i > 0 && <ResizableHandle withHandle />}
          <ResizablePanel minSize={15}>
            <PanelRenderer panel={panel} />
          </ResizablePanel>
        </div>
      ))}
    </ResizablePanelGroup>
  )
}
```

**Step 3: Create BottomArea (skeleton)**

```tsx
// packages/playground/src/components/layout/BottomArea.tsx
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'

export function BottomArea() {
  return (
    <div className="h-[250px] border-t">
      <Tabs defaultValue="tasks" className="h-full flex flex-col">
        <TabsList className="w-fit mx-3 mt-1">
          <TabsTrigger value="tasks">Tasks</TabsTrigger>
          <TabsTrigger value="events">Event History</TabsTrigger>
          <TabsTrigger value="webhooks">Webhook Logs</TabsTrigger>
        </TabsList>
        <TabsContent value="tasks" className="flex-1 overflow-auto px-3">
          <p className="text-muted-foreground text-sm">Task list will appear here</p>
        </TabsContent>
        <TabsContent value="events" className="flex-1 overflow-auto px-3">
          <p className="text-muted-foreground text-sm">Events will appear here</p>
        </TabsContent>
        <TabsContent value="webhooks" className="flex-1 overflow-auto px-3">
          <p className="text-muted-foreground text-sm">Webhook logs will appear here</p>
        </TabsContent>
      </Tabs>
    </div>
  )
}
```

**Step 4: Update App.tsx**

```tsx
// packages/playground/src/App.tsx
import { TopBar } from '@/components/layout/TopBar'
import { PanelContainer } from '@/components/layout/PanelContainer'
import { BottomArea } from '@/components/layout/BottomArea'

export function App() {
  return (
    <div className="h-screen flex flex-col bg-background text-foreground">
      <TopBar />
      <PanelContainer />
      <BottomArea />
    </div>
  )
}
```

**Step 5: Create stub panel components**

Create minimal stubs so PanelContainer can import them without errors.

```tsx
// packages/playground/src/components/panels/BackendPanel.tsx
import type { Panel } from '@/stores'
import { Card, CardHeader, CardTitle } from '@/components/ui/card'
import { usePanelStore } from '@/stores'
import { Button } from '@/components/ui/button'

export function BackendPanel({ panel }: { panel: Panel }) {
  const removePanel = usePanelStore((s) => s.removePanel)
  return (
    <div className="h-full flex flex-col p-2">
      <div className="flex items-center justify-between mb-2">
        <span className="text-sm font-medium">{panel.label}</span>
        <Button variant="ghost" size="sm" onClick={() => removePanel(panel.id)}>×</Button>
      </div>
      <div className="flex-1 text-muted-foreground text-sm flex items-center justify-center">
        Backend panel — coming soon
      </div>
    </div>
  )
}
```

```tsx
// packages/playground/src/components/panels/BrowserPanel.tsx
import type { Panel } from '@/stores'
import { usePanelStore } from '@/stores'
import { Button } from '@/components/ui/button'

export function BrowserPanel({ panel }: { panel: Panel }) {
  const removePanel = usePanelStore((s) => s.removePanel)
  return (
    <div className="h-full flex flex-col p-2">
      <div className="flex items-center justify-between mb-2">
        <span className="text-sm font-medium">{panel.label}</span>
        <Button variant="ghost" size="sm" onClick={() => removePanel(panel.id)}>×</Button>
      </div>
      <div className="flex-1 text-muted-foreground text-sm flex items-center justify-center">
        Browser panel — coming soon
      </div>
    </div>
  )
}
```

```tsx
// packages/playground/src/components/panels/WorkerPullPanel.tsx
import type { Panel } from '@/stores'
import { usePanelStore } from '@/stores'
import { Button } from '@/components/ui/button'

export function WorkerPullPanel({ panel }: { panel: Panel }) {
  const removePanel = usePanelStore((s) => s.removePanel)
  return (
    <div className="h-full flex flex-col p-2">
      <div className="flex items-center justify-between mb-2">
        <span className="text-sm font-medium">{panel.label}</span>
        <Button variant="ghost" size="sm" onClick={() => removePanel(panel.id)}>×</Button>
      </div>
      <div className="flex-1 text-muted-foreground text-sm flex items-center justify-center">
        Worker (Pull) panel — coming soon
      </div>
    </div>
  )
}
```

```tsx
// packages/playground/src/components/panels/WorkerWsPanel.tsx
import type { Panel } from '@/stores'
import { usePanelStore } from '@/stores'
import { Button } from '@/components/ui/button'

export function WorkerWsPanel({ panel }: { panel: Panel }) {
  const removePanel = usePanelStore((s) => s.removePanel)
  return (
    <div className="h-full flex flex-col p-2">
      <div className="flex items-center justify-between mb-2">
        <span className="text-sm font-medium">{panel.label}</span>
        <Button variant="ghost" size="sm" onClick={() => removePanel(panel.id)}>×</Button>
      </div>
      <div className="flex-1 text-muted-foreground text-sm flex items-center justify-center">
        Worker (WS) panel — coming soon
      </div>
    </div>
  )
}
```

**Step 6: Verify it runs**

```bash
cd packages/playground && pnpm dev
```

Open http://localhost:5173. Click "+ Add Role" → add Backend, Browser, Worker panels. Panels should appear side-by-side with resize handles. Panels should be removable with the × button.

**Step 7: Commit**

```bash
git add packages/playground/src/components packages/playground/src/App.tsx
git commit -m "feat(playground): add layout shell with TopBar, resizable panels, BottomArea"
```

---

### Task 5: API Helper Hook

Create a shared hook that provides the resolved base URL and auth headers for all panels to use.

**Files:**
- Create: `packages/playground/src/hooks/useApi.ts`

**Step 1: Create useApi hook**

```typescript
// packages/playground/src/hooks/useApi.ts
import { useCallback } from 'react'
import { useConnectionStore } from '@/stores'
import type { Panel } from '@/stores'

export function useApi(panel: Panel) {
  const { baseUrl, token } = useConnectionStore()

  const effectiveToken =
    panel.useAuth === 'none' ? undefined : panel.useAuth === 'custom' ? panel.customToken : token

  const headers = useCallback(
    (extra?: Record<string, string>): Record<string, string> => {
      const h: Record<string, string> = { 'Content-Type': 'application/json', ...extra }
      if (effectiveToken) h['Authorization'] = `Bearer ${effectiveToken}`
      return h
    },
    [effectiveToken],
  )

  const apiFetch = useCallback(
    async (path: string, init?: RequestInit) => {
      const url = `${baseUrl}${path}`
      const res = await fetch(url, {
        ...init,
        headers: { ...headers(), ...(init?.headers as Record<string, string>) },
      })
      return res
    },
    [baseUrl, headers],
  )

  return { baseUrl, effectiveToken, headers, apiFetch }
}
```

**Step 2: Commit**

```bash
git add packages/playground/src/hooks
git commit -m "feat(playground): add useApi hook for shared fetch with auth"
```

---

### Task 6: Connection Health Check

Add a health check that runs on startup and when connection settings change, updating the `connected` status.

**Files:**
- Create: `packages/playground/src/hooks/useHealthCheck.ts`
- Modify: `packages/playground/src/App.tsx`

**Step 1: Create useHealthCheck**

```typescript
// packages/playground/src/hooks/useHealthCheck.ts
import { useEffect } from 'react'
import { useConnectionStore } from '@/stores'

export function useHealthCheck() {
  const { baseUrl, setConnected } = useConnectionStore()

  useEffect(() => {
    let cancelled = false

    async function check() {
      try {
        const res = await fetch(`${baseUrl}/health`)
        if (!cancelled) setConnected(res.ok)
      } catch {
        if (!cancelled) setConnected(false)
      }
    }

    check()
    const interval = setInterval(check, 5000)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [baseUrl, setConnected])
}
```

**Step 2: Use in App.tsx**

Add `useHealthCheck()` call at the top of the `App` component.

```tsx
import { useHealthCheck } from '@/hooks/useHealthCheck'

export function App() {
  useHealthCheck()
  // ... rest unchanged
}
```

**Step 3: Verify**

Start `pnpm dev`. The badge in the TopBar should show "Connected" (green) when the embedded server is running.

**Step 4: Commit**

```bash
git add packages/playground/src/hooks/useHealthCheck.ts packages/playground/src/App.tsx
git commit -m "feat(playground): add connection health check"
```

---

### Task 7: Backend Panel

Full implementation of the Backend REST API panel.

**Files:**
- Modify: `packages/playground/src/components/panels/BackendPanel.tsx`

**Step 1: Implement BackendPanel**

The panel has 4 tabs: Create Task, Transition, Publish Event, Query.

Each tab has:
- A form with the relevant fields
- A "Send" button
- A response display area (status code + JSON body)

Key implementation details:
- Use `useApi(panel)` for all fetch calls
- JSON fields use `<Textarea>` with syntax validation
- Task ID fields use a `<Select>` populated from `useDataStore().tasks`
- Status field uses a `<Select>` with valid transition targets
- Response area shows formatted JSON with status badge (2xx green, 4xx/5xx red)

The full component is large. Structure it as a main `BackendPanel` with sub-components:
- `CreateTaskForm` — fields: type (Input), params (Textarea/JSON), ttl (Input/number), tags (Input/comma-separated), assignMode (Select), cost (Input/number)
- `TransitionForm` — fields: taskId (Select from tasks), status (Select), result (Textarea/JSON)
- `PublishEventForm` — fields: taskId (Select), type (Input), level (Select: debug/info/warn/error), data (Textarea/JSON), seriesId (Input), seriesMode (Select)
- `QueryForm` — fields: taskId (Select), action (get task / get events)
- `ResponseDisplay` — shows status code badge + formatted JSON output

API endpoints used:
- Create Task: `POST /tasks` with `CreateTaskInput` body
- Transition: `PATCH /tasks/:taskId/status` with `{ status, result?, error? }`
- Publish Event: `POST /tasks/:taskId/events` with `PublishEventInput` body
- Get Task: `GET /tasks/:taskId`
- Get Events: `GET /tasks/:taskId/events/history`

**Step 2: Verify**

Start `pnpm dev`, add a Backend panel:
1. Create a task (type: "test", ttl: 300) → should return 201 with task object
2. Transition to "running" → should return 200
3. Publish an event (type: "test.event", level: "info", data: {"msg":"hello"}) → should return 201
4. Query task details → should show the task with status "running"

**Step 3: Commit**

```bash
git add packages/playground/src/components/panels/BackendPanel.tsx
git commit -m "feat(playground): implement Backend panel with REST API forms"
```

---

### Task 8: Browser Panel

Full implementation of the Browser SSE subscription panel.

**Files:**
- Modify: `packages/playground/src/components/panels/BrowserPanel.tsx`

**Step 1: Implement BrowserPanel**

The panel has:
- **Config area**: taskId input/select, filter config (types, levels), Subscribe/Unsubscribe button
- **Status indicator**: connecting / connected / done / error
- **Event stream**: scrollable list of received SSE events, newest at bottom
- **Series accumulator**: for `accumulate` mode events, show the accumulated text live

Key implementation details:
- Use `@taskcast/client`'s `TaskcastClient` for SSE. Create a new client instance per subscription.
- Store received events in local React state (not global — each Browser panel has its own stream)
- Each event entry shows: timestamp, type, level badge, expandable data
- Auto-scroll to bottom on new events
- Connection status: use a state machine (idle → connecting → connected → done/error)
- Show `doneReason` when stream ends
- For `accumulate` series events: extract the accumulated text and show it in a dedicated area

```tsx
// Core structure:
function BrowserPanel({ panel }: { panel: Panel }) {
  const [taskId, setTaskId] = useState('')
  const [events, setEvents] = useState<SSEEnvelope[]>([])
  const [status, setStatus] = useState<'idle' | 'connecting' | 'connected' | 'done' | 'error'>('idle')
  const [doneReason, setDoneReason] = useState('')
  const [error, setError] = useState<string | null>(null)
  const clientRef = useRef<TaskcastClient | null>(null)

  // Filter state
  const [filterTypes, setFilterTypes] = useState('')
  const [filterLevels, setFilterLevels] = useState<string[]>([])

  const { baseUrl, effectiveToken } = useApi(panel)

  const subscribe = useCallback(async () => {
    setStatus('connecting')
    setEvents([])
    setError(null)

    const client = new TaskcastClient({ baseUrl, token: effectiveToken })
    clientRef.current = client

    try {
      await client.subscribe(taskId, {
        filter: {
          types: filterTypes ? filterTypes.split(',').map(s => s.trim()) : undefined,
          levels: filterLevels.length ? filterLevels as Level[] : undefined,
        },
        onEvent: (env) => {
          setStatus('connected')
          setEvents(prev => [...prev, env])
        },
        onDone: (reason) => {
          setStatus('done')
          setDoneReason(reason)
        },
        onError: (err) => {
          setStatus('error')
          setError(err.message)
        },
      })
    } catch (err) {
      setStatus('error')
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [taskId, baseUrl, effectiveToken, filterTypes, filterLevels])

  // ... render with ScrollArea, event list, status badge, etc.
}
```

**Step 2: Verify**

1. Add a Backend panel + Browser panel
2. In Backend: create a task, note the taskId
3. In Browser: enter the taskId, click Subscribe → status should show "connecting" then hold (task is pending)
4. In Backend: transition task to "running" → Browser should show "connected"
5. In Backend: publish events → Browser should show them streaming in
6. In Backend: transition to "completed" → Browser should show "done" with reason "completed"

**Step 3: Commit**

```bash
git add packages/playground/src/components/panels/BrowserPanel.tsx
git commit -m "feat(playground): implement Browser panel with SSE subscription"
```

---

### Task 9: Worker Pull Panel

Long-polling worker panel.

**Files:**
- Modify: `packages/playground/src/components/panels/WorkerPullPanel.tsx`

**Step 1: Implement WorkerPullPanel**

The panel has:
- **Config area**: workerId (auto-generated, editable), matchRule types (comma-separated), weight, timeout
- **Controls**: Start Polling / Stop Polling button
- **Status**: idle / polling / assigned
- **Current task**: when a task is claimed, show task details
- **Processing mode**: Manual / Auto toggle
  - Manual: show Transition + Publish Event buttons (like a mini Backend panel)
  - Auto: automatically transition to running, publish simulated events, then complete

Key implementation:
- Use `fetch` to call `GET /workers/pull?workerId=X&timeout=30000`
- Loop with AbortController for cancellation
- On 204: continue polling
- On 200: parse task, show details, switch to processing mode
- Need to also register the worker first if the pull endpoint requires it (check if `/workers/pull` auto-registers — based on the server code, it does register or get existing worker)

```typescript
// Core polling loop:
async function pollLoop(signal: AbortSignal) {
  while (!signal.aborted) {
    try {
      const res = await apiFetch(
        `/workers/pull?workerId=${workerId}&timeout=30000`,
        { signal }
      )
      if (res.status === 204) continue
      if (res.ok) {
        const task = await res.json()
        setCurrentTask(task)
        setStatus('assigned')
        return // stop polling, wait for processing
      }
    } catch (err) {
      if (signal.aborted) return
      // wait a bit and retry on error
      await new Promise(r => setTimeout(r, 2000))
    }
  }
}
```

**Step 2: Verify**

1. Add Backend + Worker Pull panels
2. In Worker: set matchRule types to "llm.*", click Start Polling
3. In Backend: create task with type "llm.chat", assignMode "pull"
4. Worker panel should receive the task and show details
5. Manual mode: user transitions status and publishes events via mini-forms
6. Auto mode: click "Auto Process" → automatically runs the lifecycle

**Step 3: Commit**

```bash
git add packages/playground/src/components/panels/WorkerPullPanel.tsx
git commit -m "feat(playground): implement Worker Pull panel with long-polling"
```

---

### Task 10: Worker WS Panel

WebSocket worker panel.

**Files:**
- Modify: `packages/playground/src/components/panels/WorkerWsPanel.tsx`

**Step 1: Implement WorkerWsPanel**

The panel has:
- **Config area**: matchRule types, capacity, weight
- **Controls**: Connect / Disconnect button
- **Status**: disconnected / connecting / connected / registered
- **Message log**: all WS messages sent/received (scrollable, color-coded sent/received)
- **Task offers**: when an offer/available arrives, show task summary with Accept/Reject buttons
- **Current tasks**: list of currently assigned tasks
- **Processing**: same manual/auto mode as Pull panel

Key implementation:
- Use native `WebSocket` API
- On connect, send `{ type: 'register', matchRule, capacity, weight }`
- Handle messages: `registered`, `ping` (respond with pong), `offer`, `available`, `assigned`, `error`
- Log all messages for debugging visibility

```typescript
// Core WS setup:
function connect() {
  const wsUrl = baseUrl.replace(/^http/, 'ws').replace('/taskcast', '') + '/workers/ws'
  const ws = new WebSocket(wsUrl)

  ws.onopen = () => {
    setStatus('connected')
    ws.send(JSON.stringify({
      type: 'register',
      matchRule: { taskTypes: matchTypes },
      capacity,
      weight,
    }))
  }

  ws.onmessage = (event) => {
    const msg = JSON.parse(event.data)
    addLog('received', msg)

    switch (msg.type) {
      case 'registered':
        setWorkerId(msg.workerId)
        setStatus('registered')
        break
      case 'ping':
        ws.send(JSON.stringify({ type: 'pong' }))
        break
      case 'offer':
      case 'available':
        addOffer(msg)
        break
      case 'assigned':
        // task was assigned after accept
        break
      case 'error':
        setError(msg.message)
        break
    }
  }
}
```

**Step 2: Verify**

1. Add Backend + Worker WS panels
2. In Worker: set matchRule, capacity=2, click Connect → should show "registered" with workerId
3. In Backend: create task with type matching the worker's rule, assignMode "ws-offer"
4. Worker panel should receive an offer with task details
5. Click Accept → task should be assigned
6. Process manually or auto

**Step 3: Commit**

```bash
git add packages/playground/src/components/panels/WorkerWsPanel.tsx
git commit -m "feat(playground): implement Worker WS panel with WebSocket protocol"
```

---

### Task 11: Bottom Area — Task List

Implement the Task List tab in the bottom area.

**Files:**
- Modify: `packages/playground/src/components/layout/BottomArea.tsx`
- Create: `packages/playground/src/components/bottom/TaskList.tsx`

**Step 1: Create TaskList component**

Periodically polls `GET /tasks` (or all known task IDs) to show current task states in a table.

Note: The Taskcast API doesn't have a "list all tasks" endpoint. We'll track task IDs from Backend panel operations (create/transition) in the DataStore and fetch each one individually, OR we can add a simple list endpoint to the dev server. Simplest approach: track created task IDs in the data store and poll `GET /tasks/:id` for each.

Alternative: since this is an internal tool, maintain the task list from Backend panel create responses. Every time a task is created via any panel, add its ID to the data store. Then poll individual tasks for status updates.

```tsx
// packages/playground/src/components/bottom/TaskList.tsx
import { useEffect } from 'react'
import { useDataStore, useConnectionStore } from '@/stores'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'

const statusColors: Record<string, string> = {
  pending: 'bg-yellow-500',
  assigned: 'bg-blue-500',
  running: 'bg-green-500',
  completed: 'bg-gray-500',
  failed: 'bg-red-500',
  timeout: 'bg-orange-500',
  cancelled: 'bg-gray-400',
}

export function TaskList() {
  const { tasks, setTasks } = useDataStore()
  const { baseUrl } = useConnectionStore()

  // Poll known tasks for status updates
  useEffect(() => {
    const interval = setInterval(async () => {
      if (tasks.length === 0) return
      const updated = await Promise.all(
        tasks.map(async (t) => {
          try {
            const res = await fetch(`${baseUrl}/tasks/${t.id}`)
            if (res.ok) return await res.json()
            return t
          } catch {
            return t
          }
        }),
      )
      setTasks(updated)
    }, 2000)
    return () => clearInterval(interval)
  }, [tasks.length, baseUrl, setTasks])

  return (
    <ScrollArea className="h-full">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b text-left">
            <th className="p-2">ID</th>
            <th className="p-2">Type</th>
            <th className="p-2">Status</th>
            <th className="p-2">Worker</th>
            <th className="p-2">Created</th>
          </tr>
        </thead>
        <tbody>
          {tasks.map((task) => (
            <tr key={task.id} className="border-b hover:bg-muted/50">
              <td className="p-2 font-mono text-xs">{task.id.slice(-8)}</td>
              <td className="p-2">{task.type ?? '—'}</td>
              <td className="p-2">
                <Badge variant="outline">{task.status}</Badge>
              </td>
              <td className="p-2 text-xs">{task.assignedWorker ?? '—'}</td>
              <td className="p-2 text-xs">
                {new Date(task.createdAt).toLocaleTimeString()}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {tasks.length === 0 && (
        <p className="p-4 text-center text-muted-foreground text-sm">
          No tasks yet. Create one from a Backend panel.
        </p>
      )}
    </ScrollArea>
  )
}
```

**Step 2: Wire up to BottomArea**

Import `TaskList` and render it in the "tasks" TabsContent.

**Step 3: Connect Backend panel to data store**

When Backend panel creates a task successfully, add the task to `useDataStore().tasks`. The simplest way: after a successful `POST /tasks` or `PATCH /tasks/:id/status`, call `setTasks` to update/add the returned task.

**Step 4: Verify**

Create tasks from Backend panel. Bottom area should show them in the task list with auto-updating status.

**Step 5: Commit**

```bash
git add packages/playground/src/components/bottom packages/playground/src/components/layout/BottomArea.tsx
git commit -m "feat(playground): implement Task List in bottom area"
```

---

### Task 12: Bottom Area — Event History

Implement the Event History tab.

**Files:**
- Create: `packages/playground/src/components/bottom/EventHistory.tsx`
- Modify: `packages/playground/src/components/layout/BottomArea.tsx`

**Step 1: Create EventHistory component**

Shows all events from all tasks in reverse-chronological order. Events are added to `useDataStore().globalEvents` from Backend panel publish operations and optionally from Browser panel SSE streams.

```tsx
// packages/playground/src/components/bottom/EventHistory.tsx
import { useDataStore } from '@/stores'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'

const levelColors = {
  debug: 'secondary',
  info: 'default',
  warn: 'outline',
  error: 'destructive',
} as const

export function EventHistory() {
  const events = useDataStore((s) => s.globalEvents)

  return (
    <ScrollArea className="h-full">
      <div className="space-y-1 p-1">
        {events.map((ev) => (
          <div key={ev.id} className="flex items-start gap-2 text-xs p-1 hover:bg-muted/50 rounded">
            <span className="text-muted-foreground whitespace-nowrap">
              {new Date(ev.timestamp).toLocaleTimeString()}
            </span>
            <Badge variant={levelColors[ev.level] ?? 'secondary'} className="text-[10px]">
              {ev.level}
            </Badge>
            <span className="font-mono">{ev.type}</span>
            <span className="text-muted-foreground font-mono truncate">
              {JSON.stringify(ev.data)}
            </span>
            {ev.seriesId && (
              <Badge variant="outline" className="text-[10px]">
                {ev.seriesId}:{ev.seriesMode}
              </Badge>
            )}
          </div>
        ))}
        {events.length === 0 && (
          <p className="p-4 text-center text-muted-foreground text-sm">
            No events yet.
          </p>
        )}
      </div>
    </ScrollArea>
  )
}
```

**Step 2: Wire up to BottomArea, connect panels to addEvent**

**Step 3: Commit**

```bash
git add packages/playground/src/components/bottom/EventHistory.tsx packages/playground/src/components/layout/BottomArea.tsx
git commit -m "feat(playground): implement Event History in bottom area"
```

---

### Task 13: Bottom Area — Webhook Logs

Implement the Webhook Logs tab.

**Files:**
- Create: `packages/playground/src/components/bottom/WebhookLogs.tsx`
- Modify: `packages/playground/src/components/layout/BottomArea.tsx`

**Step 1: Create WebhookLogs component**

Shows webhook delivery attempts. Since webhooks require a target URL that the playground likely can't receive, this tab primarily shows the concept. For embedded mode, we can intercept webhook deliveries from the engine's webhook system.

For v1: keep it simple — show a table of webhook logs from the data store. The Backend panel could have a "Register Webhook" sub-tab that calls the webhook registration API, and the dev server could log webhook deliveries to a special endpoint that the playground polls.

Simpler approach for v1: just show the structure with a placeholder message about how webhooks work. Can be fully implemented in a follow-up.

```tsx
// packages/playground/src/components/bottom/WebhookLogs.tsx
import { useDataStore } from '@/stores'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Badge } from '@/components/ui/badge'

export function WebhookLogs() {
  const logs = useDataStore((s) => s.webhookLogs)

  return (
    <ScrollArea className="h-full">
      {logs.length === 0 ? (
        <p className="p-4 text-center text-muted-foreground text-sm">
          No webhook deliveries yet. Register a webhook via the Backend panel to see deliveries here.
        </p>
      ) : (
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b text-left">
              <th className="p-2">Time</th>
              <th className="p-2">URL</th>
              <th className="p-2">Status</th>
              <th className="p-2">Payload</th>
            </tr>
          </thead>
          <tbody>
            {logs.map((log) => (
              <tr key={log.id} className="border-b hover:bg-muted/50">
                <td className="p-2 text-xs">{new Date(log.timestamp).toLocaleTimeString()}</td>
                <td className="p-2 text-xs font-mono truncate max-w-[200px]">{log.url}</td>
                <td className="p-2">
                  <Badge variant={log.statusCode && log.statusCode < 400 ? 'default' : 'destructive'}>
                    {log.statusCode ?? 'ERR'}
                  </Badge>
                </td>
                <td className="p-2 text-xs font-mono truncate max-w-[300px]">
                  {JSON.stringify(log.payload)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </ScrollArea>
  )
}
```

**Step 2: Wire up to BottomArea**

**Step 3: Commit**

```bash
git add packages/playground/src/components/bottom/WebhookLogs.tsx packages/playground/src/components/layout/BottomArea.tsx
git commit -m "feat(playground): implement Webhook Logs in bottom area"
```

---

### Task 14: Per-Panel Auth Configuration

Add the ability for each panel to override the global auth token.

**Files:**
- Create: `packages/playground/src/components/panels/PanelAuthConfig.tsx`
- Modify: each panel component to include `<PanelAuthConfig>`

**Step 1: Create PanelAuthConfig component**

A small dropdown/popover in each panel header that lets users choose auth mode:
- Global (default) — use the token from TopBar
- Custom — enter a custom token for this panel
- None — send no auth header

```tsx
// packages/playground/src/components/panels/PanelAuthConfig.tsx
import { usePanelStore } from '@/stores'
import type { Panel } from '@/stores'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Input } from '@/components/ui/input'

export function PanelAuthConfig({ panel }: { panel: Panel }) {
  const updatePanel = usePanelStore((s) => s.updatePanel)

  return (
    <div className="flex items-center gap-1">
      <Select
        value={panel.useAuth}
        onValueChange={(v) => updatePanel(panel.id, { useAuth: v as Panel['useAuth'] })}
      >
        <SelectTrigger className="h-6 w-[90px] text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="global">Global</SelectItem>
          <SelectItem value="custom">Custom</SelectItem>
          <SelectItem value="none">No Auth</SelectItem>
        </SelectContent>
      </Select>
      {panel.useAuth === 'custom' && (
        <Input
          className="h-6 w-[140px] text-xs"
          placeholder="Bearer token"
          type="password"
          value={panel.customToken ?? ''}
          onChange={(e) => updatePanel(panel.id, { customToken: e.target.value })}
        />
      )}
    </div>
  )
}
```

**Step 2: Add to each panel header**

In each panel component, add `<PanelAuthConfig panel={panel} />` next to the panel label in the header.

**Step 3: Verify**

Each panel should show the auth selector. Switching to "Custom" shows an input. Switching to "None" should send requests without auth headers.

**Step 4: Commit**

```bash
git add packages/playground/src/components/panels/PanelAuthConfig.tsx
git commit -m "feat(playground): add per-panel auth configuration"
```

---

### Task 15: Integration Testing & Polish

Manual end-to-end verification of the full playground workflow.

**Step 1: Full workflow test**

Run `pnpm dev` and test the complete scenario:

1. Open http://localhost:5173
2. Verify TopBar shows "Embedded" mode, "Connected" status
3. Add all 4 panel types
4. **Backend panel**: Create a task (type: "llm.chat", assignMode: "pull", ttl: 600)
5. **Browser panel**: Enter the task ID, subscribe → should show "connecting" (task is pending)
6. **Worker Pull panel**: Start polling with matchRule "llm.*" → should receive the task
7. Worker panel: transition to "running" → Browser panel should start showing "connected"
8. Worker panel: publish events (type: "llm.delta", data: {delta: "Hello"}, seriesId: "response", seriesMode: "accumulate")
9. Browser panel: should show streaming events
10. Worker panel: transition to "completed"
11. Browser panel: should show "done" reason: "completed"
12. Bottom area: Task list should show the task as "completed"
13. Bottom area: Event history should show all events

Then test WS worker:
14. Create another task with assignMode: "ws-offer"
15. Add a Worker WS panel, connect, register
16. Should receive an offer → accept → process → complete

**Step 2: Fix any issues found**

Address layout problems, styling issues, or functional bugs.

**Step 3: Final commit**

```bash
git add -A packages/playground
git commit -m "feat(playground): polish and integration fixes"
```

---

## Summary

| Task | Description | Key Files |
|------|-------------|-----------|
| 1 | Package scaffolding | package.json, configs, shadcn init |
| 2 | Dev server | dev-server/server.ts |
| 3 | Zustand stores | stores/connection, panels, data |
| 4 | Layout shell | TopBar, PanelContainer, BottomArea, stub panels |
| 5 | useApi hook | hooks/useApi.ts |
| 6 | Health check | hooks/useHealthCheck.ts |
| 7 | Backend panel | panels/BackendPanel.tsx |
| 8 | Browser panel | panels/BrowserPanel.tsx |
| 9 | Worker Pull panel | panels/WorkerPullPanel.tsx |
| 10 | Worker WS panel | panels/WorkerWsPanel.tsx |
| 11 | Task List bottom | bottom/TaskList.tsx |
| 12 | Event History bottom | bottom/EventHistory.tsx |
| 13 | Webhook Logs bottom | bottom/WebhookLogs.tsx |
| 14 | Per-panel auth | panels/PanelAuthConfig.tsx |
| 15 | Integration test & polish | Manual E2E verification |
