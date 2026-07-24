export type DependencyName = 'redisCommand' | 'redisPubSub' | 'postgres'
export type DependencyState =
  | 'starting'
  | 'healthy'
  | 'reconnecting'
  | 'unhealthy'
export type DependencyErrorKind =
  | 'connection_refused'
  | 'connection_reset'
  | 'timeout'
  | 'dns'
  | 'authentication'
  | 'connection_closed'
  | 'unavailable'

export interface DependencyObservation {
  dependency: DependencyName
  state: Exclude<DependencyState, 'starting'>
  errorKind?: DependencyErrorKind
  attempt?: number
  nextRetryMs?: number
}

export interface DependencyObserver {
  observe(observation: DependencyObservation): void
}

export class DependencyUnavailableError extends Error {
  override readonly cause: unknown

  constructor(
    public readonly dependency: DependencyName,
    public readonly kind: DependencyErrorKind,
    cause?: unknown,
  ) {
    super(`${dependency} unavailable (${kind})`)
    this.name = 'DependencyUnavailableError'
    this.cause = cause
  }
}

export function findDependencyUnavailableError(
  value: unknown,
): DependencyUnavailableError | undefined {
  const seen = new Set<unknown>()
  let current = value
  while (current instanceof Error && !seen.has(current)) {
    if (current instanceof DependencyUnavailableError) return current
    seen.add(current)
    current = (current as Error & { cause?: unknown }).cause
  }
  return undefined
}
