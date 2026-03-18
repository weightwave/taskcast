import { Command } from 'commander'
import { existsSync } from 'fs'
import { join, dirname } from 'path'
import { createRequire } from 'module'

export function registerPlaygroundCommand(program: Command): void {
  program
    .command('playground')
    .description('Serve only the playground UI (no engine)')
    .option('-p, --port <port>', 'port to listen on', '5173')
    .action(async (options: { port: string }) => {
      const port = Number(options.port)
      try {
        const require = createRequire(import.meta.url)
        const pkgPath = require.resolve('@taskcast/playground/package.json')
        const distDir = join(dirname(pkgPath), 'dist')
        if (!existsSync(distDir)) {
          console.error('[taskcast] Playground dist not found. Run `pnpm --filter @taskcast/playground build` first.')
          process.exit(1)
        }
        const { OpenAPIHono } = await import('@hono/zod-openapi')
        const app = new OpenAPIHono()
        const { serveStatic } = await import('@hono/node-server/serve-static')
        app.use('/_playground/*', serveStatic({ root: distDir, rewriteRequestPath: (p: string) => p.replace(/^\/_playground/, '') }))
        app.get('/_playground/*', serveStatic({ root: distDir, rewriteRequestPath: () => '/index.html' }))
        app.get('/', (c: { redirect: (url: string) => Response }) => c.redirect('/_playground/'))
        const { serve } = await import('@hono/node-server')
        serve({ fetch: app.fetch, port }, () => {
          console.log(`[taskcast] Playground UI at http://localhost:${port}/_playground/`)
          console.log('[taskcast] Use "External" mode in the UI to connect to a remote server.')
        })
      } catch {
        console.error('[taskcast] @taskcast/playground not available.')
        process.exit(1)
      }
    })
}
