import { Command } from 'commander'
import { NodeConfigManager } from '../node-config.js'
import { createClientFromNodeAsync } from '../client.js'
import type { TaskcastServerClient } from '@taskcast/server-sdk'

export interface TaskListItem {
  id: string
  type?: string
  status: string
  createdAt: number
}

export function formatTaskList(tasks: TaskListItem[]): string {
  if (tasks.length === 0) return 'No tasks found.'

  const header = `${'ID'.padEnd(28)}${'TYPE'.padEnd(13)}${'STATUS'.padEnd(11)}CREATED`
  const rows = tasks.map((t) => {
    const id = (t.id ?? '').padEnd(28)
    const type = (t.type ?? '').padEnd(13)
    const status = (t.status ?? '').padEnd(11)
    const created = formatTimestamp(t.createdAt)
    return `${id}${type}${status}${created}`
  })

  return [header, ...rows].join('\n')
}

export function formatTaskInspect(task: any, events: any[]): string {
  const lines: string[] = []

  lines.push(`Task: ${task.id}`)
  lines.push(`  Type:    ${task.type ?? ''}`)
  lines.push(`  Status:  ${task.status}`)
  lines.push(`  Params:  ${task.params != null ? JSON.stringify(task.params) : ''}`)
  lines.push(`  Created: ${formatTimestamp(task.createdAt)}`)

  if (events.length > 0) {
    const last5 = events.slice(-5)
    lines.push('')
    lines.push(`Recent Events (last ${last5.length}):`)
    for (let i = 0; i < last5.length; i++) {
      const e = last5[i]
      const series = e.seriesId ? `series:${e.seriesId}` : ''
      const ts = formatTimestamp(e.timestamp)
      lines.push(`  #${i}  ${(e.type ?? '').padEnd(13)}${(e.level ?? '').padEnd(7)}${series.padEnd(17)}${ts}`)
    }
  } else {
    lines.push('')
    lines.push('No events.')
  }

  return lines.join('\n')
}

function formatTimestamp(ts: number): string {
  if (!ts) return ''
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

/**
 * Call _request on the server-sdk client.
 * The SDK doesn't expose a listTasks method, so we use the internal _request
 * to leverage the already-configured auth token and fetch function.
 */
async function clientRequest<T>(client: TaskcastServerClient, method: string, path: string): Promise<T> {
  // Access the private _request method via bracket notation
  return (client as any)._request(method, path) as Promise<T>
}

export function registerTasksCommand(program: Command): void {
  const tasks = program
    .command('tasks')
    .description('Manage tasks on a Taskcast server')

  tasks
    .command('list')
    .description('List tasks')
    .option('--status <status>', 'Filter by status (e.g. running)')
    .option('--type <type>', 'Filter by task type (e.g. llm.*)')
    .option('--limit <limit>', 'Maximum number of tasks to show', '20')
    .option('--node <name>', 'Named node to query')
    .action(async (opts: { status?: string; type?: string; limit?: string; node?: string }) => {
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

      try {
        const client = await createClientFromNodeAsync(node)

        const params = new URLSearchParams()
        if (opts.status) params.set('status', opts.status)
        if (opts.type) params.set('type', opts.type)
        const qs = params.toString()
        const path = `/tasks${qs ? `?${qs}` : ''}`

        const body = await clientRequest<{ tasks: TaskListItem[] }>(client, 'GET', path)
        const limit = parseInt(opts.limit ?? '20', 10)
        const tasks = body.tasks.slice(0, limit)
        console.log(formatTaskList(tasks))
      } catch (err) {
        console.error(`Error: ${(err as Error).message}`)
        process.exit(1)
      }
    })

  tasks
    .command('inspect')
    .description('Inspect a task and its recent events')
    .argument('<taskId>', 'Task ID to inspect')
    .option('--node <name>', 'Named node to query')
    .action(async (taskId: string, opts: { node?: string }) => {
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

      try {
        const client = await createClientFromNodeAsync(node)
        const task = await client.getTask(taskId)
        const events = await client.getHistory(taskId)
        console.log(formatTaskInspect(task, events))
      } catch (err) {
        console.error(`Error: ${(err as Error).message}`)
        process.exit(1)
      }
    })
}
