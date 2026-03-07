/**
 * Starts two servers for browser E2E tests:
 * 1. Taskcast API server on port 3799 (auth: none, workers enabled)
 * 2. Dashboard UI server on port 3722 (static files + /api/config for auto-connect)
 */

import { serve } from '@hono/node-server'
import { Hono } from 'hono'
import { cors } from 'hono/cors'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import type { ShortTermStore, BroadcastProvider } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { dashboardDistPath } from '@taskcast/dashboard-web/dist-path'
import { existsSync, readFileSync, statSync } from 'fs'
import { join, extname, resolve } from 'path'

// --- 1. Start Taskcast API server on port 3799 ---
const shortTermStore: ShortTermStore = new MemoryShortTermStore()
const broadcast: BroadcastProvider = new MemoryBroadcastProvider()
const engine = new TaskEngine({ shortTermStore, broadcast })
const workerManager = new WorkerManager({ engine, shortTermStore, broadcast })

const { app: apiApp, stop } = createTaskcastApp({
  engine,
  shortTermStore,
  auth: { mode: 'none' },
  workerManager,
})

// Enable CORS so the dashboard (port 3722) can reach the API (port 3799)
const wrappedApiApp = new Hono()
wrappedApiApp.use('*', cors({ origin: '*' }))
wrappedApiApp.route('/', apiApp as unknown as Hono)

const apiServer = serve({ fetch: wrappedApiApp.fetch, port: 3799 }, () => {
  console.log('[e2e] API server started on http://localhost:3799')
})

// --- 2. Start Dashboard UI server on port 3722 ---

if (!existsSync(dashboardDistPath)) {
  console.error('[e2e] Dashboard dist not found. Run: pnpm --filter @taskcast/dashboard-web build')
  process.exit(1)
}

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html',
  '.js': 'application/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.eot': 'application/vnd.ms-fontobject',
  '.map': 'application/json',
}

const dashboardApp = new Hono()

// Auto-connect config endpoint (auth: none, no token needed)
dashboardApp.get('/api/config', (c) => {
  return c.json({
    baseUrl: 'http://localhost:3799',
  })
})

// Serve static dashboard files with SPA fallback
dashboardApp.get('*', (c) => {
  const urlPath = decodeURIComponent(new URL(c.req.url).pathname)
  const resolved = resolve(join(dashboardDistPath, urlPath))

  // Path traversal protection
  if (!resolved.startsWith(dashboardDistPath)) {
    return c.text('Not Found', 404)
  }

  let filePath = resolved

  // Try the exact file first, then fall back to index.html (SPA)
  const isFile = existsSync(filePath) && statSync(filePath).isFile()
  if (!isFile) {
    filePath = join(dashboardDistPath, 'index.html')
  }

  if (!existsSync(filePath)) {
    return c.text('Not Found', 404)
  }

  const ext = extname(filePath)
  const contentType = MIME_TYPES[ext] ?? 'application/octet-stream'
  const body = readFileSync(filePath)
  return c.body(body, 200, { 'Content-Type': contentType })
})

const dashboardServer = serve({ fetch: dashboardApp.fetch, port: 3722 }, () => {
  console.log('[e2e] Dashboard UI started on http://localhost:3722')
})

// Cleanup on shutdown
function shutdown() {
  stop()
  apiServer.close()
  dashboardServer.close()
  process.exit(0)
}

process.on('SIGTERM', shutdown)
process.on('SIGINT', shutdown)