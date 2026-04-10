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
 * Returns `host:port/dbname`. On parse failure:
 *   - returns `<redacted>` if the raw string contains `@` (which indicates
 *     possible embedded credentials), so a malformed URL can never leak
 *     user:pass into a log line;
 *   - otherwise returns the raw string unchanged, which is safe because it
 *     cannot contain userinfo.
 */
export function formatDisplayUrl(pgUrl: string): string {
  try {
    const parsed = new URL(pgUrl)
    const dbname = parsed.pathname.replace(/^\//, '') || 'postgres'
    return `${parsed.hostname}:${parsed.port || '5432'}/${dbname}`
  } catch {
    return pgUrl.includes('@') ? '<redacted>' : pgUrl
  }
}
