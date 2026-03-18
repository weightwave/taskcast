import { Command } from 'commander'

export function registerUiCommand(program: Command): void {
  program
    .command('ui')
    .alias('dashboard')
    .description('Start the Taskcast Dashboard web UI')
    .option('-p, --port <port>', 'Dashboard port', '3722')
    .option('-s, --server <url>', 'Taskcast server URL', 'http://localhost:3721')
    .option('--admin-token <token>', 'Admin token for auto-connect')
    .action(async (opts: { port: string; server: string; adminToken?: string }) => {
      const { dashboardDistPath } = await import('@taskcast/dashboard-web/dist-path')
      const { Hono } = await import('hono')
      const { serve } = await import('@hono/node-server')
      const { existsSync, readFileSync, statSync } = await import('fs')
      const { join: joinPath, extname, resolve: resolvePath } = await import('path')

      if (!existsSync(dashboardDistPath)) {
        console.error(
          '[taskcast] Dashboard not built. Run: pnpm --filter @taskcast/dashboard-web build',
        )
        process.exit(1)
      }

      // Exchange admin token for JWT at startup (never expose raw token to browser)
      let dashboardJwt: string | undefined
      if (opts.adminToken) {
        try {
          const tokenRes = await fetch(`${opts.server}/admin/token`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ adminToken: opts.adminToken }),
          })
          if (tokenRes.ok) {
            const { token } = await tokenRes.json()
            dashboardJwt = token
          } else if (tokenRes.status === 404) {
            // Admin API not enabled — server may be in auth: none mode
            console.log('[taskcast] Admin API not enabled, connecting without JWT')
          } else {
            console.error(`[taskcast] Failed to exchange admin token: ${tokenRes.status}`)
            process.exit(1)
          }
        } catch (err) {
          console.error(`[taskcast] Cannot reach server at ${opts.server}: ${err instanceof Error ? err.message : err}`)
          process.exit(1)
        }
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

      const app = new Hono()

      // Auto-connect config endpoint (never exposes admin token — only pre-exchanged JWT)
      app.get('/api/config', (c) => {
        return c.json({
          baseUrl: opts.server,
          token: dashboardJwt,
        })
      })

      // Serve static dashboard files with SPA fallback
      app.get('*', (c) => {
        const urlPath = decodeURIComponent(new URL(c.req.url).pathname)
        const resolved = resolvePath(joinPath(dashboardDistPath, urlPath))

        // Path traversal protection: ensure resolved path stays within dashboard dist
        if (!resolved.startsWith(dashboardDistPath)) {
          return c.text('Not Found', 404)
        }

        let filePath = resolved

        // Try the exact file first, then fall back to index.html (SPA)
        const isFile = existsSync(filePath) && statSync(filePath).isFile()
        if (!isFile) {
          filePath = joinPath(dashboardDistPath, 'index.html')
        }

        if (!existsSync(filePath)) {
          return c.text('Not Found', 404)
        }

        const ext = extname(filePath)
        const contentType = MIME_TYPES[ext] ?? 'application/octet-stream'
        const body = readFileSync(filePath)
        return c.body(body, 200, { 'Content-Type': contentType })
      })

      const port = Number(opts.port)
      serve({ fetch: app.fetch, port }, () => {
        console.log(`[taskcast] Dashboard running at http://localhost:${port}`)
        console.log(`[taskcast] Connected to server: ${opts.server}`)
        if (opts.adminToken) {
          console.log(`[taskcast] Admin token provided for auto-connect`)
        }
      })
    })
}
