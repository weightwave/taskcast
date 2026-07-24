import type postgres from 'postgres'
import type { DependencyErrorKind } from '@taskcast/core'

function classifyPostgresCode(
  code: string,
): DependencyErrorKind | undefined {
  switch (code) {
    case 'ECONNREFUSED':
      return 'connection_refused'
    case 'ECONNRESET':
    case 'EPIPE':
      return 'connection_reset'
    case 'ETIMEDOUT':
    case 'ESOCKETTIMEDOUT':
      return 'timeout'
    case 'ENOTFOUND':
    case 'EAI_AGAIN':
      return 'dns'
    case 'CONNECTION_CLOSED':
      return 'connection_closed'
    default:
      if (
        (code.length === 5 && code.startsWith('08'))
        || code === '57P01'
      ) {
        return 'unavailable'
      }
      return undefined
  }
}

export function classifyPostgresConnectivity(
  error: unknown,
): DependencyErrorKind | undefined {
  const seen = new Set<unknown>()
  let current = error

  while (
    typeof current === 'object'
    && current !== null
    && !seen.has(current)
  ) {
    seen.add(current)
    const candidate = current as { code?: unknown, cause?: unknown }
    if (typeof candidate.code === 'string') {
      const kind = classifyPostgresCode(candidate.code)
      if (kind) return kind
    }
    current = candidate.cause
  }

  return undefined
}

export async function postgresCheck(
  sql: ReturnType<typeof postgres>,
): Promise<void> {
  await sql`SELECT 1`
}
