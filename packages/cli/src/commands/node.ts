import { Command } from 'commander'
import { NodeConfigManager } from '../node-config.js'
import type { NodeListEntry } from '../node-config.js'

export function formatNodeList(nodes: NodeListEntry[]): string {
  if (nodes.length === 0) {
    return 'No nodes configured. Using default: http://localhost:3721'
  }

  return nodes.map(n => {
    const marker = n.current ? '*' : ' '
    const tokenInfo = n.tokenType ? ` (${n.tokenType})` : ''
    return `${marker} ${n.name}  ${n.url}${tokenInfo}`
  }).join('\n')
}

export function registerNodeCommand(program: Command): void {
  const node = program
    .command('node')
    .description('Manage Taskcast server connections')

  node
    .command('add <name>')
    .description('Add a named node connection')
    .requiredOption('--url <url>', 'Server URL')
    .option('--token <token>', 'Authentication token')
    .option('--token-type <type>', 'Token type: jwt or admin', 'jwt')
    .action((name: string, opts: { url: string; token?: string; tokenType?: string }) => {
      const mgr = new NodeConfigManager()
      mgr.add(name, {
        url: opts.url,
        token: opts.token,
        tokenType: opts.token ? (opts.tokenType as 'jwt' | 'admin') : undefined,
      })
      console.log(`Added node "${name}" → ${opts.url}`)
    })

  node
    .command('remove <name>')
    .description('Remove a named node connection')
    .action((name: string) => {
      const mgr = new NodeConfigManager()
      mgr.remove(name)
      console.log(`Removed node "${name}"`)
    })

  node
    .command('use <name>')
    .description('Set the current active node')
    .action((name: string) => {
      const mgr = new NodeConfigManager()
      mgr.use(name)
      console.log(`Switched to node "${name}"`)
    })

  node
    .command('list')
    .description('List all configured nodes')
    .action(() => {
      const mgr = new NodeConfigManager()
      const nodes = mgr.list()
      console.log(formatNodeList(nodes))
    })
}
