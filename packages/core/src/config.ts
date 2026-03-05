import { load as yamlLoad } from 'js-yaml'

export interface TaskcastConfig {
  port?: number
  logLevel?: 'debug' | 'info' | 'warn' | 'error'
  auth?: {
    mode: 'none' | 'jwt' | 'custom'
    jwt?: {
      algorithm?: string
      secret?: string
      publicKey?: string
      publicKeyFile?: string
      issuer?: string
      audience?: string
    }
  }
  adapters?: {
    broadcast?: { provider: string; url?: string }
    shortTermStore?: { provider: string; url?: string }
    longTermStore?: { provider: string; url?: string }
  }
  sentry?: {
    dsn?: string
    captureTaskFailures?: boolean
    captureTaskTimeouts?: boolean
    captureUnhandledErrors?: boolean
    captureDroppedEvents?: boolean
    captureStorageErrors?: boolean
    captureBroadcastErrors?: boolean
    traceSSEConnections?: boolean
    traceEventPublish?: boolean
  }
  webhook?: {
    defaultRetry?: {
      retries?: number
      backoff?: 'fixed' | 'exponential' | 'linear'
      initialDelayMs?: number
      maxDelayMs?: number
      timeoutMs?: number
    }
  }
  cleanup?: {
    rules?: unknown[]
  }
  workers?: {
    enabled?: boolean
    defaults?: {
      assignMode?: 'external' | 'pull' | 'ws-offer' | 'ws-race'
      heartbeatIntervalMs?: number
      heartbeatTimeoutMs?: number
      offerTimeoutMs?: number
      disconnectPolicy?: 'reassign' | 'mark' | 'fail'
      disconnectGraceMs?: number
    }
  }
}

export function interpolateEnvVars(value: string): string {
  return value.replace(/\$\{([^}]+)\}/g, (_match, varName: string) => {
    return process.env[varName] ?? _match
  })
}

function interpolateObject(obj: unknown): unknown {
  if (typeof obj === 'string') return interpolateEnvVars(obj)
  /* v8 ignore next -- arrays in config are supported but not exercised in unit tests */
  if (Array.isArray(obj)) return obj.map(interpolateObject)
  if (obj !== null && typeof obj === 'object') {
    return Object.fromEntries(
      Object.entries(obj as Record<string, unknown>).map(([k, v]) => [k, interpolateObject(v)])
    )
  }
  return obj
}

export function parseConfig(content: string, format: 'json' | 'yaml'): TaskcastConfig {
  let raw: unknown
  if (format === 'json') {
    raw = JSON.parse(content)
  } else {
    const interpolated = interpolateEnvVars(content)
    raw = yamlLoad(interpolated)
  }
  const config = interpolateObject(raw) as TaskcastConfig
  // Coerce port to number if it's a string (from env var interpolation)
  if (typeof config.port === 'string') {
    const n = parseInt(config.port, 10)
    if (!isNaN(n)) config.port = n
    else delete (config as Record<string, unknown>)['port']
  }
  return config
}

export interface ConfigLoadResult {
  config: TaskcastConfig
  source: 'explicit' | 'local' | 'global' | 'none'
}

export async function loadConfigFile(
  configPath?: string,
  globalConfigDir?: string,
): Promise<ConfigLoadResult> {
  const { readFileSync, existsSync } = await import('fs')
  const { resolve, extname, join } = await import('path')
  const { homedir } = await import('os')

  // 1. Explicit path
  if (configPath) {
    const fullPath = resolve(configPath)
    if (!existsSync(fullPath)) return { config: {}, source: 'explicit' }

    const ext = extname(fullPath).toLowerCase()
    /* v8 ignore next 4 -- dynamic import of .ts/.js/.mjs config files */
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return { config: mod.default ?? {}, source: 'explicit' }
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'explicit' }
  }

  // 2. Local directory
  const localCandidates = [
    'taskcast.config.ts',
    'taskcast.config.js',
    'taskcast.config.mjs',
    'taskcast.config.yaml',
    'taskcast.config.yml',
    'taskcast.config.json',
  ]

  for (const candidate of localCandidates) {
    const fullPath = resolve(candidate)
    if (!existsSync(fullPath)) continue

    const ext = extname(fullPath).toLowerCase()
    /* v8 ignore next 4 -- dynamic import of .ts/.js/.mjs config files */
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return { config: mod.default ?? {}, source: 'local' }
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'local' }
  }

  // 3. Global directory (~/.taskcast/) — only static formats
  const globalDir = globalConfigDir ?? join(homedir(), '.taskcast')
  const globalCandidates = [
    'taskcast.config.yaml',
    'taskcast.config.yml',
    'taskcast.config.json',
  ]

  for (const candidate of globalCandidates) {
    const fullPath = join(globalDir, candidate)
    if (!existsSync(fullPath)) continue

    const content = readFileSync(fullPath, 'utf8')
    const ext = extname(fullPath).toLowerCase()
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'global' }
  }

  return { config: {}, source: 'none' }
}
