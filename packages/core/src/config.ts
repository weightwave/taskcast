import { load as yamlLoad } from 'js-yaml'
import { ulid } from 'ulidx'
import type { PermissionScope } from './types.js'

export interface TrustedServiceConfig {
  name: string
  key: string
  taskIds?: string[] | '*'
  scope: PermissionScope[]
}

export interface TaskcastConfig {
  port?: number
  logLevel?: 'debug' | 'info' | 'warn' | 'error'
  adminToken?: string
  /** Enable the admin API endpoint (POST /admin/token). Defaults to false. */
  adminApi?: boolean
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
  trustedServices?: TrustedServiceConfig[]
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
  storageLifecycle?: StorageLifecycleConfig
}

export interface StorageLifecycleConfig {
  hotRetentionEnabled?: boolean
  hotRetentionTerminalSeconds?: number
  hotRetentionIdleSeconds?: number
  rehydrateReplayEvents?: number
  storageLockTtlSeconds?: number
  ttlSweepIntervalSeconds?: number
  ttlSweepBatchSize?: number
}

export interface ResolvedStorageLifecycleConfig {
  hotRetentionEnabled: boolean
  hotRetentionTerminalSeconds: number
  hotRetentionIdleSeconds: number
  rehydrateReplayEvents: number
  storageLockTtlSeconds: number
  ttlSweepIntervalSeconds: number
  ttlSweepBatchSize: number
}

const STORAGE_LIFECYCLE_DEFAULTS: ResolvedStorageLifecycleConfig = {
  hotRetentionEnabled: false,
  hotRetentionTerminalSeconds: 86_400,
  hotRetentionIdleSeconds: 3_600,
  rehydrateReplayEvents: 1_000,
  storageLockTtlSeconds: 30,
  ttlSweepIntervalSeconds: 5,
  ttlSweepBatchSize: 100,
}
const MAX_STORAGE_LIFECYCLE_SECONDS = Math.floor(Number.MAX_SAFE_INTEGER / 1_000)

export function resolveStorageLifecycleConfig(
  config: TaskcastConfig,
  env: Record<string, string | undefined> = process.env,
): ResolvedStorageLifecycleConfig {
  const file = config.storageLifecycle ?? {}
  const positiveInteger = (
    key: string,
    envValue: string | undefined,
    fileValue: number | undefined,
    fallback: number,
    maximum = Number.MAX_SAFE_INTEGER,
  ): number => {
    const value: unknown = envValue !== undefined ? envValue : fileValue ?? fallback
    const parsed = typeof value === 'number' ? value : Number(value)
    if (!Number.isSafeInteger(parsed) || parsed <= 0) {
      throw new Error(`${key} must be a positive integer`)
    }
    if (parsed > maximum) {
      throw new Error(
        `${key} must be a positive integer no greater than ${maximum}`,
      )
    }
    return parsed
  }
  const retentionEnv = env['TASKCAST_HOT_RETENTION_ENABLED']
  const fileRetentionEnabled: unknown = file.hotRetentionEnabled
  if (
    fileRetentionEnabled !== undefined
    && typeof fileRetentionEnabled !== 'boolean'
  ) {
    throw new Error('storageLifecycle.hotRetentionEnabled must be true or false')
  }
  let hotRetentionEnabled = fileRetentionEnabled
    ?? STORAGE_LIFECYCLE_DEFAULTS.hotRetentionEnabled
  if (retentionEnv !== undefined) {
    if (retentionEnv !== 'true' && retentionEnv !== 'false') {
      throw new Error('TASKCAST_HOT_RETENTION_ENABLED must be true or false')
    }
    hotRetentionEnabled = retentionEnv === 'true'
  }

  return {
    hotRetentionEnabled,
    hotRetentionTerminalSeconds: positiveInteger(
      'TASKCAST_HOT_RETENTION_TERMINAL_SECONDS',
      env['TASKCAST_HOT_RETENTION_TERMINAL_SECONDS'],
      file.hotRetentionTerminalSeconds,
      STORAGE_LIFECYCLE_DEFAULTS.hotRetentionTerminalSeconds,
      MAX_STORAGE_LIFECYCLE_SECONDS,
    ),
    hotRetentionIdleSeconds: positiveInteger(
      'TASKCAST_HOT_RETENTION_IDLE_SECONDS',
      env['TASKCAST_HOT_RETENTION_IDLE_SECONDS'],
      file.hotRetentionIdleSeconds,
      STORAGE_LIFECYCLE_DEFAULTS.hotRetentionIdleSeconds,
      MAX_STORAGE_LIFECYCLE_SECONDS,
    ),
    rehydrateReplayEvents: positiveInteger(
      'TASKCAST_REHYDRATE_REPLAY_EVENTS',
      env['TASKCAST_REHYDRATE_REPLAY_EVENTS'],
      file.rehydrateReplayEvents,
      STORAGE_LIFECYCLE_DEFAULTS.rehydrateReplayEvents,
    ),
    storageLockTtlSeconds: positiveInteger(
      'TASKCAST_STORAGE_LOCK_TTL_SECONDS',
      env['TASKCAST_STORAGE_LOCK_TTL_SECONDS'],
      file.storageLockTtlSeconds,
      STORAGE_LIFECYCLE_DEFAULTS.storageLockTtlSeconds,
      MAX_STORAGE_LIFECYCLE_SECONDS,
    ),
    ttlSweepIntervalSeconds: positiveInteger(
      'TASKCAST_TTL_SWEEP_INTERVAL_SECONDS',
      env['TASKCAST_TTL_SWEEP_INTERVAL_SECONDS'],
      file.ttlSweepIntervalSeconds,
      STORAGE_LIFECYCLE_DEFAULTS.ttlSweepIntervalSeconds,
      MAX_STORAGE_LIFECYCLE_SECONDS,
    ),
    ttlSweepBatchSize: positiveInteger(
      'TASKCAST_TTL_SWEEP_BATCH_SIZE',
      env['TASKCAST_TTL_SWEEP_BATCH_SIZE'],
      file.ttlSweepBatchSize,
      STORAGE_LIFECYCLE_DEFAULTS.ttlSweepBatchSize,
    ),
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
  /** Resolved absolute path to the config file that was loaded. Undefined when source is 'none'. */
  path?: string
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
    if (!existsSync(fullPath)) return { config: {}, source: 'explicit', path: fullPath }

    const ext = extname(fullPath).toLowerCase()
    /* v8 ignore next 4 -- dynamic import of .ts/.js/.mjs config files */
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return { config: mod.default ?? {}, source: 'explicit', path: fullPath }
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'explicit', path: fullPath }
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

    /* v8 ignore start -- local config file loading is tested only in integration (requires real files on disk) */
    const ext = extname(fullPath).toLowerCase()
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return { config: mod.default ?? {}, source: 'local', path: fullPath }
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'local', path: fullPath }
    /* v8 ignore stop */
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
    return { config: parseConfig(content, format), source: 'global', path: fullPath }
  }

  return { config: {}, source: 'none' }
}

/**
 * Resolves the admin token based on config.
 * - If `adminApi` is false/unset, returns null (admin API disabled, no token needed).
 * - If `adminApi` is true and `adminToken` is set, returns it.
 * - If `adminApi` is true and `adminToken` is not set, auto-generates a ULID and logs it.
 * Mutates the config in place.
 */
export function resolveAdminToken(config: TaskcastConfig): string | null {
  if (!config.adminApi) {
    return null
  }
  if (!config.adminToken) {
    const token = ulid()
    config.adminToken = token
    console.log(`[taskcast] Admin token (auto-generated): ${token}`)
    return token
  }
  return config.adminToken
}
