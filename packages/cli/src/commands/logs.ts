import { Command } from 'commander'
import { NodeConfigManager } from '../node-config.js'

// ─── Formatting ──────────────────────────────────────────────────────────────

export function formatEvent(
  event: { type: string; level: string; timestamp: number; data: unknown },
  taskId?: string,
): string {
  const time = new Date(event.timestamp).toLocaleTimeString('en-US', { hour12: false })
  const taskPrefix = taskId ? `${taskId.slice(0, 7)}..  ` : ''

  if (event.type === 'taskcast:done' || event.type === 'taskcast.done') {
    const reason =
      typeof event.data === 'object' && event.data !== null
        ? (event.data as Record<string, unknown>).reason ?? 'unknown'
        : 'unknown'
    return `[${time}] ${taskPrefix}[DONE] ${reason}`
  }

  const dataStr = JSON.stringify(event.data)
  return `[${time}] ${taskPrefix}${event.type.padEnd(16)} ${event.level.padEnd(5)} ${dataStr}`
}

// ─── SSE Consumer ────────────────────────────────────────────────────────────

export async function consumeSSE(
  url: string,
  token: string | undefined,
  onEvent: (event: Record<string, unknown>, sseEventName: string) => void,
  onDone?: () => void,
  fetchFn: typeof fetch = fetch,
): Promise<void> {
  const headers: Record<string, string> = { Accept: 'text/event-stream' }
  if (token) headers['Authorization'] = `Bearer ${token}`

  const res = await fetchFn(url, { headers })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  if (!res.body) throw new Error('No response body')

  const reader = (res.body as ReadableStream<Uint8Array>).getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let currentEvent = ''
  let currentData = ''

  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })

    // Parse SSE format: event: xxx\ndata: xxx\n\n
    const lines = buffer.split('\n')
    buffer = lines.pop()! // incomplete line stays in buffer

    for (const line of lines) {
      if (line.startsWith('event:')) {
        currentEvent = line.slice(6).trim()
      } else if (line.startsWith('data:')) {
        currentData = line.slice(5).trim()
      } else if (line === '') {
        // Empty line = end of event
        if (currentData) {
          try {
            const parsed = JSON.parse(currentData)
            onEvent(parsed, currentEvent)
            if (currentEvent === 'taskcast.done') onDone?.()
          } catch {
            // skip unparseable data
          }
        }
        currentEvent = ''
        currentData = ''
      }
    }
  }
}

// ─── Token Resolution ────────────────────────────────────────────────────────

async function resolveToken(
  node: { url: string; token?: string; tokenType?: string },
  fetchFn: typeof fetch = fetch,
): Promise<string | undefined> {
  if (node.tokenType === 'admin' && node.token) {
    const res = await fetchFn(`${node.url}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: node.token }),
    })
    if (!res.ok) throw new Error(`Admin token exchange failed: HTTP ${res.status}`)
    const body = (await res.json()) as { token: string }
    return body.token
  }
  return node.tokenType === 'jwt' ? node.token : node.token
}

// ─── Commands ────────────────────────────────────────────────────────────────

export function registerLogsCommand(program: Command): void {
  program
    .command('logs <taskId>')
    .description('Stream events from a task in real-time')
    .option('--types <types>', 'Filter by event types (CSV, supports wildcards)')
    .option('--levels <levels>', 'Filter by levels (CSV)')
    .option('--node <name>', 'Target node')
    .action(async (taskId: string, opts: { types?: string; levels?: string; node?: string }) => {
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

      const params = new URLSearchParams()
      if (opts.types) params.set('types', opts.types)
      if (opts.levels) params.set('levels', opts.levels)
      const qs = params.toString()
      const url = `${node.url}/tasks/${taskId}/events${qs ? `?${qs}` : ''}`

      try {
        const token = await resolveToken(node)
        await consumeSSE(
          url,
          token,
          (event, sseEventName) => {
            if (sseEventName === 'taskcast.done') {
              console.log(formatEvent({
                type: 'taskcast.done',
                level: 'info',
                timestamp: Date.now(),
                data: event,
              }))
            } else if (sseEventName === 'taskcast.event') {
              console.log(formatEvent(event as { type: string; level: string; timestamp: number; data: unknown }))
            }
          },
          () => process.exit(0),
        )
      } catch (err) {
        console.error(`Error: ${(err as Error).message}`)
        process.exit(1)
      }
    })
}

export function registerTailCommand(program: Command): void {
  program
    .command('tail')
    .description('Stream events from all tasks in real-time')
    .option('--types <types>', 'Filter by event types (CSV, supports wildcards)')
    .option('--levels <levels>', 'Filter by levels (CSV)')
    .option('--node <name>', 'Target node')
    .action(async (opts: { types?: string; levels?: string; node?: string }) => {
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

      const params = new URLSearchParams()
      if (opts.types) params.set('types', opts.types)
      if (opts.levels) params.set('levels', opts.levels)
      const qs = params.toString()
      const url = `${node.url}/events${qs ? `?${qs}` : ''}`

      try {
        const token = await resolveToken(node)
        await consumeSSE(
          url,
          token,
          (event, sseEventName) => {
            if (sseEventName === 'taskcast.event') {
              const e = event as { type: string; level: string; timestamp: number; data: unknown; taskId: string }
              console.log(formatEvent(e, e.taskId))
            }
          },
        )
      } catch (err) {
        console.error(`Error: ${(err as Error).message}`)
        process.exit(1)
      }
    })
}
