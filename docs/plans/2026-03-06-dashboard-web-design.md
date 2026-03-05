# @taskcast/dashboard-web — Design Document

**Date:** 2026-03-06
**Status:** Draft

## Overview

`@taskcast/dashboard-web` is a production-ready management dashboard for Taskcast. It connects to a deployed Taskcast Server to provide real-time monitoring and administrative control over tasks, workers, and event streams.

**Goals:**
- Real-time monitoring: task status, worker health, event flow
- Administrative operations: create/cancel tasks, manage workers, view history
- Published to npm as `@taskcast/dashboard-web`
- Deployable via CLI (`taskcast ui`), Docker, or static CDN hosting

## Tech Stack

| Technology | Purpose |
|-----------|---------|
| React 18 | UI framework |
| Vite | Build & dev server |
| shadcn/ui + Tailwind CSS | Component library & styling |
| Zustand | UI state (connection config, layout preferences) |
| TanStack Query | Server state (tasks, workers, events — caching, polling, pagination) |
| `@taskcast/server-sdk` | REST API calls |
| `@taskcast/client` | SSE real-time subscriptions |

## Architecture

```
@taskcast/dashboard-web
├── src/
│   ├── main.tsx
│   ├── App.tsx
│   ├── components/
│   │   ├── ui/                 # shadcn/ui base components
│   │   ├── layout/             # Shell, sidebar, header
│   │   ├── tasks/              # Task list, detail, create dialog
│   │   ├── workers/            # Worker list, detail, capacity bars
│   │   ├── events/             # Event stream, timeline, filters
│   │   └── overview/           # Dashboard cards, stats
│   ├── pages/
│   │   ├── overview.tsx        # Overview dashboard
│   │   ├── tasks.tsx           # Task list + detail
│   │   ├── events.tsx          # Real-time event stream
│   │   └── workers.tsx         # Worker management
│   ├── stores/
│   │   └── connection.ts       # Zustand: connection config
│   ├── hooks/
│   │   ├── use-tasks.ts        # TanStack Query: task data
│   │   ├── use-workers.ts      # TanStack Query: worker data
│   │   ├── use-events.ts       # SSE subscription + history
│   │   └── use-admin-auth.ts   # Admin token → JWT exchange
│   ├── lib/
│   │   ├── api.ts              # TaskcastServerClient wrapper
│   │   └── utils.ts            # Shared utilities
│   └── types/
│       └── dashboard.ts        # Dashboard-specific types
├── public/
├── index.html
├── vite.config.ts
├── tailwind.config.ts
├── tsconfig.json
└── package.json
```

### Deployment Modes

| Mode | How | Details |
|------|-----|---------|
| CLI embedded | `npx @taskcast/cli ui` | CLI serves built static files via Hono `serveStatic`, auto-injects server URL |
| Docker | `docker run mwr1998/taskcast-dashboard` | nginx serves static files, user configures server URL via env var |
| Static hosting | Deploy `dist/` to CDN | User enters server URL manually on first load |

### CLI Integration

When `taskcast ui` is used:
- CLI serves the dashboard's `dist/` directory as static files
- CLI exposes `GET /api/config` returning `{ baseUrl, adminToken? }` so the dashboard can auto-connect
- The dashboard-web package exports its dist path for CLI to reference:
  ```typescript
  // packages/dashboard-web/src/index.ts
  export const distPath = new URL('../dist', import.meta.url).pathname
  ```

## Authentication

### Admin Token Flow

```
┌──────────┐     adminToken      ┌──────────────┐      JWT        ┌───────────┐
│Dashboard │ ──────────────────→ │ POST         │ ──────────────→ │ All API   │
│  Login   │                     │ /admin/token │                 │ Requests  │
└──────────┘                     └──────────────┘                 └───────────┘
```

1. **Server startup**: Admin token is either configured (`adminToken` in config) or auto-generated as UUID and printed to terminal
2. **Dashboard login**: User enters admin token in login screen
3. **Token exchange**: `POST /admin/token { adminToken, scopes?: PermissionScope[] }` → returns JWT with requested scopes (default: `["*"]`)
4. **API access**: All subsequent API calls use the JWT in `Authorization: Bearer` header
5. **Storage**: JWT stored in `localStorage`, admin token is NOT stored

### Server-Side Changes Required

New route: `POST /admin/token`

```typescript
// Request
interface AdminTokenRequest {
  adminToken: string
  scopes?: PermissionScope[]  // default: ["*"]
  expiresIn?: number          // JWT expiry in seconds, default: 24h
}

// Response
interface AdminTokenResponse {
  token: string   // JWT
  expiresAt: number
}
```

New config field:
```typescript
interface TaskcastConfig {
  // ... existing fields
  adminToken?: string  // If not set, auto-generate UUID on startup
}
```

### Connection Store

```typescript
interface ConnectionStore {
  baseUrl: string         // Server URL
  jwt: string | null      // JWT from admin token exchange
  connected: boolean
  connect: (url: string, adminToken: string) => Promise<void>
  disconnect: () => void
}
```

## Pages

### 1. Overview Dashboard

Real-time system overview with auto-refreshing stats (5s polling via TanStack Query).

**Content:**
- **Status cards**: Task count by status (pending, running, completed, failed, etc.) with color-coded badges
- **Worker summary**: Online count, total capacity, total used slots
- **Recent tasks**: Latest 10 tasks in a compact table
- **System health**: Server connectivity status

### 2. Task List + Detail

**List view:**
- Table: ID, type, status (badge), assignedWorker, hot/cold indicator, subscriberCount, createdAt, updatedAt
- Filters: status dropdown, type search, tags
- Sorting: by createdAt (default), updatedAt
- Auto-refresh via TanStack Query polling (3s)

**Detail panel** (slide-out or side panel on task click):
- **Header**: Task ID, status badge, hot/cold indicator, subscriber count
- **Info section**: type, params (JSON viewer), result/error (JSON viewer), TTL, tags, assignMode, assignedWorker, cost
- **Timestamps**: created, updated, completed
- **Event timeline**: Historical events + live SSE stream (hybrid view)
  - Events shown as cards with type, level badge, timestamp, expandable data
  - Series accumulation view for `accumulate` mode
- **Actions**: Status transition buttons (dynamically shown based on state machine — e.g., running task shows "Complete", "Fail", "Cancel")

**Create task dialog:**
- Form: type, params (JSON editor), TTL, tags, assignMode
- Submit → `POST /tasks`

### 3. Real-Time Event Stream

Dedicated page for monitoring live events across tasks.

- **Task selector**: Pick a task ID to subscribe (dropdown or type-ahead)
- **Filter controls**: Type patterns (e.g., `llm.*`), level checkboxes (debug/info/warn/error)
- **Live stream**: SSE subscription via `@taskcast/client`, events rendered as timeline entries
- **Event cards**: Type, level (color badge), timestamp, expandable JSON data view
- **Series view**: For `accumulate` series, show merged text growing in real-time
- **Connection indicator**: Connecting / streaming / done / error states

### 4. Worker Management

**Worker list:**
- Table: ID, status (badge), capacity bar (`usedSlots/capacity` as progress bar), connectionMode, weight, connectedAt, lastHeartbeat
- Resource metrics: If worker reports resource data in metadata (CPU, memory, etc.), display as optional columns or expandable detail
- Auto-refresh (5s polling)

**Worker actions:**
- **Pause assignment**: Set worker to `draining` status (finishes current tasks, no new assignments)
- **Resume**: Set worker back from `draining` to `idle`
- **Force disconnect**: `DELETE /workers/:id` with confirmation dialog

**Worker detail** (expandable row or side panel):
- Current task assignments
- Resource data from metadata (if available — CPU, memory, custom metrics displayed as key-value pairs)
- Connection info: mode, weight, matchRule

## Server-Side Changes Required

These changes are prerequisites for the dashboard and affect both TypeScript and Rust implementations.

### New API Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/admin/token` | POST | Exchange admin token for JWT |
| `/api/config` | GET | CLI-mode only: return auto-connect config |

### Enhanced Response Fields

| Endpoint | New Fields | Source |
|----------|-----------|--------|
| `GET /tasks/:taskId` | `hot: boolean` | Check if task exists in ShortTermStore |
| `GET /tasks/:taskId` | `subscriberCount: number` | Count active SSE connections for this task |
| `GET /tasks` (list) | Same fields on each task | Same sources |

### Worker Status Management

| Endpoint | Change |
|----------|--------|
| `PATCH /workers/:id/status` | New endpoint: set worker status (e.g., `draining`) |

### New Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `adminToken` | `string?` | Auto-generated UUID | Token for dashboard admin access |

## Package Configuration

```json
{
  "name": "@taskcast/dashboard-web",
  "version": "0.3.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview"
  },
  "exports": {
    ".": {
      "import": "./src/index.ts"
    },
    "./dist": {
      "import": "./dist/index.html"
    }
  }
}
```

### Dependencies

| Dependency | Purpose |
|-----------|---------|
| `@taskcast/server-sdk` | REST API client |
| `@taskcast/client` | SSE subscriptions |
| `@taskcast/core` | Type definitions (Task, TaskEvent, etc.) |
| `react`, `react-dom` | UI framework |
| `zustand` | UI state management |
| `@tanstack/react-query` | Server state management |
| `tailwindcss`, `@tailwindcss/vite` | Styling |
| shadcn/ui components | Table, Card, Badge, Button, Dialog, Tabs, Progress, ScrollArea, Input, Select, DropdownMenu, Sheet |
| `vite` | Build tool |
| `react-router-dom` | Client-side routing |

## UI Layout

```
┌──────────────────────────────────────────────────────────────┐
│  ◆ Taskcast Dashboard    [Server: http://...]    [Disconnect]│
├────────┬─────────────────────────────────────────────────────┤
│        │                                                     │
│  📊    │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐  │
│Overview│  │ Pending  │ │ Running │ │Complete │ │ Failed  │  │
│        │  │   12     │ │    5    │ │   847   │ │    3    │  │
│  📋    │  └─────────┘ └─────────┘ └─────────┘ └─────────┘  │
│ Tasks  │                                                     │
│        │  Workers: 8 online  │  Capacity: 24/40 (60%)        │
│  📡    │                                                     │
│Events  │  ┌─────────────────────────────────────────────┐    │
│        │  │ Recent Tasks                                │    │
│  ⚙️    │  │ task-abc  llm.chat    ● running   2s ago    │    │
│Workers │  │ task-def  tool.exec   ● completed 5s ago    │    │
│        │  │ task-ghi  batch.proc  ○ pending   12s ago   │    │
│        │  └─────────────────────────────────────────────┘    │
└────────┴─────────────────────────────────────────────────────┘
```

- Left: collapsible sidebar with navigation
- Main area: page content
- Responsive (but desktop-first for v1)

## Data Flow

```
TanStack Query (polling)
  ├── useTasksQuery()     → GET /tasks           → task list + hot/cold + subscriberCount
  ├── useTaskQuery(id)    → GET /tasks/:id        → single task detail
  ├── useWorkersQuery()   → GET /workers          → worker list + metadata
  └── useEventsQuery(id)  → GET /tasks/:id/events/history → event history

SSE Subscription (real-time)
  └── useEventStream(id)  → @taskcast/client      → live event stream

Mutations
  ├── useCreateTask()     → POST /tasks
  ├── useTransitionTask() → PATCH /tasks/:id/status
  ├── useDrainWorker()    → PATCH /workers/:id/status
  └── useDisconnectWorker() → DELETE /workers/:id
```

## Non-Goals (v1)

- No SSR / server-side rendering
- No i18n (English UI only)
- No mobile-optimized layout (desktop-first)
- No webhook management UI (future)
- No task creation with advanced scheduling
- No historical analytics / charts (future)
- No multi-server management (connects to one server at a time)