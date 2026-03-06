/**
 * Resolve a Postgres connection URL from multiple sources.
 *
 * Priority: --url flag > environment variable > config file value.
 */
export function resolvePostgresUrl(options: {
  url?: string | undefined
  envUrl?: string | undefined
  configUrl?: string | undefined
}): string | undefined {
  return options.url ?? options.envUrl ?? options.configUrl
}

/**
 * Format a Postgres URL for display, stripping credentials.
 *
 * Returns `host:port/dbname`. Falls back to the raw string on parse failure.
 */
export function formatDisplayUrl(pgUrl: string): string {
  try {
    const parsed = new URL(pgUrl)
    const dbname = parsed.pathname.replace(/^\//, '') || 'postgres'
    return `${parsed.hostname}:${parsed.port || '5432'}/${dbname}`
  } catch {
    return pgUrl
  }
}
