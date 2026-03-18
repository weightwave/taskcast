import { Command } from 'commander'
import { NodeConfigManager } from '../node-config.js'

export interface PingResult {
  ok: boolean
  latencyMs?: number
  error?: string
}

export async function pingServer(url: string, fetchFn: typeof fetch = fetch): Promise<PingResult> {
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
    .option('--node <name>', 'Named node to ping')
    .action(async (opts: { node?: string }) => {
      const mgr = new NodeConfigManager()
      let node
      if (opts.node) {
        node = mgr.get(opts.node)
        if (!node) {
          console.error(`Node "${opts.node}" not found`)
          process.exit(1)
        }
      } else {
        node = mgr.getCurrent()
      }

      const result = await pingServer(node.url)
      if (result.ok) {
        console.log(`OK — taskcast at ${node.url} (${result.latencyMs}ms)`)
      } else {
        console.error(`FAIL — cannot reach ${node.url}: ${result.error}`)
        process.exit(1)
      }
    })
}
