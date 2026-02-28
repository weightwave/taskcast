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
    shortTerm?: { provider: string; url?: string }
    longTerm?: { provider: string; url?: string }
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

export async function loadConfigFile(configPath?: string): Promise<TaskcastConfig> {
  const { readFileSync, existsSync } = await import('fs')
  const { resolve, extname } = await import('path')

  const candidates = configPath
    ? [configPath]
    : [
        'taskcast.config.ts',
        'taskcast.config.js',
        'taskcast.config.mjs',
        'taskcast.config.yaml',
        'taskcast.config.yml',
        'taskcast.config.json',
      ]

  for (const candidate of candidates) {
    const fullPath = resolve(candidate)
    if (!existsSync(fullPath)) continue

    const ext = extname(fullPath).toLowerCase()
    /* v8 ignore next 4 -- dynamic import of .ts/.js/.mjs config files; not exercised in unit tests */
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return mod.default ?? {}
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return parseConfig(content, format)
  }

  return {}
}
