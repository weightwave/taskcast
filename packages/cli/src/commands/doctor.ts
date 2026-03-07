import { Command } from 'commander'
import { NodeConfigManager } from '../node-config.js'
import type { NodeEntry } from '../node-config.js'

export interface DoctorResult {
  server: { ok: boolean; url: string; uptime?: number; error?: string }
  auth: { status: 'ok' | 'warn'; mode?: string; message?: string }
  adapters: Record<string, { provider: string; status: string }>
}

export async function runDoctor(
  node: NodeEntry,
  fetchFn?: typeof fetch,
): Promise<DoctorResult> {
  try {
    const res = await (fetchFn ?? fetch)(`${node.url}/health/detail`)
    if (!res.ok) {
      return {
        server: { ok: false, url: node.url, error: `HTTP ${res.status}` },
        auth: { status: 'warn' },
        adapters: {},
      }
    }
    const body = await res.json()

    const authStatus = (!node.token && body.auth.mode !== 'none') ? 'warn' : 'ok'
    const authMessage = authStatus === 'warn' ? 'no token configured for this node' : undefined

    return {
      server: { ok: true, url: node.url, uptime: body.uptime },
      auth: { status: authStatus, mode: body.auth.mode, message: authMessage },
      adapters: body.adapters,
    }
  } catch (err) {
    return {
      server: { ok: false, url: node.url, error: (err as Error).message },
      auth: { status: 'warn' },
      adapters: {},
    }
  }
}

export function formatDoctorResult(result: DoctorResult): string {
  const lines: string[] = []

  // Server line
  if (result.server.ok) {
    const uptimeStr = result.server.uptime != null ? ` (uptime: ${result.server.uptime}s)` : ''
    lines.push(`Server:    OK  taskcast at ${result.server.url}${uptimeStr}`)
  } else {
    lines.push(`Server:    FAIL  cannot reach ${result.server.url}: ${result.server.error}`)
  }

  // Auth line
  if (result.auth.status === 'ok') {
    lines.push(`Auth:      OK  ${result.auth.mode}`)
  } else {
    const msg = result.auth.message ?? (result.auth.mode ?? 'unknown')
    lines.push(`Auth:      WARN  ${msg}`)
  }

  // Adapter lines
  const adapterNames = ['broadcast', 'shortTermStore', 'longTermStore'] as const
  const labelMap: Record<string, string> = {
    broadcast: 'Broadcast',
    shortTermStore: 'ShortTerm',
    longTermStore: 'LongTerm',
  }

  for (const name of adapterNames) {
    const adapter = result.adapters[name]
    const label = `${labelMap[name]}:`.padEnd(11)
    if (adapter) {
      const statusTag = adapter.status === 'ok' ? 'OK' : 'FAIL'
      lines.push(`${label}${statusTag}  ${adapter.provider}`)
    } else if (name === 'longTermStore') {
      lines.push(`${label}SKIP  not configured`)
    }
  }

  return lines.join('\n')
}

export function registerDoctorCommand(program: Command): void {
  program
    .command('doctor')
    .description('Deep health check against a Taskcast server')
    .option('--node <name>', 'Node name to check (default: current node)')
    .action(async (opts: { node?: string }) => {
      const mgr = new NodeConfigManager()
      let node: NodeEntry

      if (opts.node) {
        const found = mgr.get(opts.node)
        if (!found) {
          console.error(`Node "${opts.node}" not found`)
          process.exit(1)
        }
        node = found
      } else {
        node = mgr.getCurrent()
      }

      const result = await runDoctor(node)
      console.log(formatDoctorResult(result))

      if (!result.server.ok) {
        process.exit(1)
      }
    })
}
