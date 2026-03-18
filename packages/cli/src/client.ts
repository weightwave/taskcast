import { TaskcastServerClient } from '@taskcast/server-sdk'
import type { NodeEntry } from './node-config.js'

/**
 * Creates a TaskcastServerClient synchronously from a NodeEntry.
 * Suitable for JWT or no-auth nodes. Admin token nodes should use
 * createClientFromNodeAsync instead.
 */
export function createClientFromNode(node: NodeEntry, fetchFn?: typeof fetch): TaskcastServerClient {
  const opts: { baseUrl: string; token?: string; fetch?: typeof fetch } = { baseUrl: node.url }
  if (node.tokenType === 'jwt' && node.token) opts.token = node.token
  if (fetchFn) opts.fetch = fetchFn
  return new TaskcastServerClient(opts)
}

/**
 * Creates a TaskcastServerClient asynchronously from a NodeEntry.
 * Handles admin token exchange: POSTs to {url}/admin/token to get a JWT,
 * then creates the client with the exchanged JWT.
 */
export async function createClientFromNodeAsync(node: NodeEntry, fetchFn?: typeof fetch): Promise<TaskcastServerClient> {
  if (node.tokenType === 'admin' && node.token) {
    const f = fetchFn ?? globalThis.fetch
    const res = await f(`${node.url}/admin/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: node.token }),
    })

    if (!res.ok) {
      let message = `HTTP ${res.status}`
      try {
        const err = (await res.json()) as { error?: string }
        message = err.error ?? message
      } catch {
        // ignore parse errors
      }
      throw new Error(message)
    }

    const { token: jwt } = (await res.json()) as { token: string }
    const opts: { baseUrl: string; token?: string; fetch?: typeof fetch } = { baseUrl: node.url, token: jwt }
    if (fetchFn) opts.fetch = fetchFn
    return new TaskcastServerClient(opts)
  }

  return createClientFromNode(node, fetchFn)
}
