# @taskcast/playground — Design Document

**Date:** 2026-03-06
**Status:** Approved

## Overview

`@taskcast/playground` is an interactive multi-role debugging and demo tool for Taskcast. It allows users to simulate all participants in the Taskcast ecosystem — backends, browsers, and workers — from a single web interface, observing their real-time interactions through the Taskcast service.

**Goals:**
- Development debugging: test task lifecycles, SSE behavior, worker protocols
- User demo: showcase Taskcast capabilities and interaction patterns
- Not published to npm (`private: true`)

## Tech Stack

| Technology | Purpose |
|-----------|---------|
| React 18 | UI framework |
| Vite | Build & dev server |
| shadcn/ui + Tailwind CSS | Component library & styling |
| Zustand | State management |
| `@taskcast/client` | SSE subscriptions in Browser panels |
| `@taskcast/server` + `@taskcast/core` | Embedded Taskcast server |
| tsx | Dev server runner |

## Architecture

```
@taskcast/playground
├── dev-server/              # Node dev server (embedded Taskcast instance)
│   └── server.ts            # Starts embedded Taskcast + Vite dev middleware
├── src/                     # React SPA
│   ├── App.tsx
│   ├── components/
│   │   ├── panels/          # Role panel components
│   │   │   ├── BackendPanel.tsx
│   │   │   ├── BrowserPanel.tsx
│   │   │   ├── WorkerPullPanel.tsx
│   │   │   └── WorkerWsPanel.tsx
│   │   ├── layout/          # Top bar, panel container, bottom area
│   │   │   ├── TopBar.tsx
│   │   │   ├── PanelContainer.tsx
│   │   │   └── BottomArea.tsx
│   │   └── ui/              # shadcn/ui components
│   ├── hooks/               # Custom hooks
│   ├── stores/              # Zustand stores
│   │   ├── connection.ts
│   │   ├── panels.ts
│   │   └── data.ts
│   └── lib/                 # Utility functions
├── package.json
├── vite.config.ts
├── tailwind.config.ts
├── tsconfig.json
└── index.html
```

### Startup

```bash
cd packages/playground && pnpm dev
```

This starts:
1. An embedded Taskcast HTTP server (memory adapters, port 3721)
2. Vite dev server (port 5173, proxying `/taskcast` → 3721)

Users can switch to "external server" mode and provide any Taskcast service URL.

## UI Layout

```
┌──────────────────────────────────────────────────────────┐
│  Server: [Embedded ▼]  http://localhost:3721  [Connect…] │
│  Auth: [Global token input]                              │
├──────────────────────────────────────────────────────────┤
│ [+ Add Role]  [Backend] [Browser①] [Browser②] [Worker①] │
├────────────┬────────────┬────────────┬───────────────────┤
│  Backend   │ Browser ①  │ Browser ②  │  Worker ①         │
│            │            │            │                   │
│ POST task  │ SSE sub    │ SSE sub    │ WS connected      │
│ PATCH stat │ events ↓   │ events ↓   │ waiting offer…    │
│ POST event │ {delta:"H"}│ {delta:"H"}│ task-123 accepted │
│            │ {delta:"e"}│ {delta:"e"}│ → running         │
│ [Send]     │ {done}     │ {done}     │ → completed       │
├────────────┴────────────┴────────────┴───────────────────┤
│ [Tasks] [Event History] [Webhook Logs]                   │
│ ┌─ Task list table / Event timeline / Webhook log ─────┐ │
│ └──────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
```

- Top bar: server connection config + auth
- Middle: resizable role panels (shadcn `ResizablePanelGroup`)
- Bottom: tabbed global views (Tasks, Events, Webhooks)

## Panel Types

### Backend Panel

REST API caller with form-based interface.

**Tabs:**
1. **Create Task** — Form: type, params (JSON editor), ttl, tags, assignMode → POST
2. **Transition** — Select taskId (dropdown), target status, result (JSON) → PATCH
3. **Publish Event** — Select taskId, type, level, data (JSON), seriesId, seriesMode → POST
4. **Query** — View task details, event history

Each operation shows: editable request body → send → response status + body. Like Postman but specialized for Taskcast API.

### Browser Panel

SSE subscription visualizer.

1. Enter taskId or pick from task list dropdown
2. Configure filter (types, levels, since)
3. Click "Subscribe" → establish SSE connection
4. Real-time event stream display with collapsible entries
5. Connection status indicator (connecting / connected / done / error)
6. Live series accumulation view (e.g., streaming text appearing character by character)

### Worker (Pull) Panel

Long-polling worker simulator.

1. Set workerId, matchRule (types/tags)
2. Click "Start Polling" → loop long-poll requests
3. On task received, display task details
4. Processing mode:
   - **Manual** — user manually triggers transitions and event publishing
   - **Auto** — simulated processing: auto running → publish events → completed

### Worker (WS) Panel

WebSocket worker simulator.

1. Click "Connect" → WebSocket connection
2. Send register message (matchRule, capacity)
3. On offer received, display task info
4. Click accept/reject
5. After accept, same manual/auto processing as Pull worker

## State Management

### Zustand Stores

```typescript
// Connection state
interface ConnectionStore {
  mode: 'embedded' | 'external'
  baseUrl: string
  token?: string
  connected: boolean
}

// Panel instances
interface PanelStore {
  panels: Panel[]
  addPanel(type: PanelType): void
  removePanel(id: string): void
}

// Global data
interface DataStore {
  tasks: Task[]
  globalEvents: TaskEvent[]
  webhookLogs: WebhookLog[]
}
```

### Data Flow

```
Backend panel ── fetch POST ──→ Taskcast Server ──→ Task state change
                                      ↓
                                 Broadcast
                                      ↓
Browser panel ←── SSE stream ──── Event push
Worker panel  ←── WS/Pull ─────── Task dispatch
Bottom tabs   ←── polling ──────── Task list + event history
```

Each panel operates independently — Backend panels use `fetch`, Browser panels use `@taskcast/client` SSE, Worker panels use native fetch/WebSocket. All interactions go through the Taskcast service, authentically simulating distributed scenarios.

### Auth per Panel

Each panel can:
- Use the global token (default)
- Use a custom token (for testing permission isolation)
- Use no token (testing unauthenticated mode)

## Bottom Global Area

Three tabs:

1. **Task List** — Table of all tasks with status, auto-refreshing
2. **Event History** — Global event timeline, reverse chronological, with type/level filters
3. **Webhook Logs** — Webhook delivery records (URL, payload, status code, retry attempts)

## Package Configuration

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
  }
}
```

### Dependencies

| Dependency | Purpose |
|-----------|---------|
| `@taskcast/core` | Type definitions |
| `@taskcast/server` | Embedded server |
| `@taskcast/client` | Browser panel SSE subscriptions |
| `@taskcast/react` | Optional reuse of useTaskEvents |
| `react`, `react-dom` | UI framework |
| `zustand` | State management |
| `tailwindcss`, `@tailwindcss/vite` | Styling |
| shadcn/ui components | ResizablePanel, Tabs, Button, Input, Select, Card, Badge, ScrollArea |
| `vite` | Build tool |
| `tsx` | Dev server runner |
| `hono`, `@hono/node-server` | Embedded Taskcast server |

### No Testing Required

As a private dev tool, no coverage thresholds or unit tests. The playground itself is a testing tool.

## Non-Goals (v1)

- No SSR / server-side rendering
- No persistent state (localStorage save/restore can be added later)
- No i18n (English UI only for v1)
- No mobile-responsive layout
