import {
  DependencyUnavailableError,
  findDependencyUnavailableError,
  type DependencyErrorKind,
  type DependencyName,
  type DependencyObservation,
  type DependencyObserver,
  type DependencyState,
} from '@taskcast/core'

export type DependencyCheck = () => Promise<void>
export type DependencyHealthLogger = (record: Record<string, unknown>) => void

export interface DependencySnapshot {
  configured: true
  state: DependencyState
  lastTransitionAt: string
  lastErrorKind?: DependencyErrorKind
  consecutiveFailures: number
  reconnectAttempts?: number
}

export interface ReadinessResult {
  ok: boolean
  dependencies: Partial<Record<
    DependencyName,
    { state: DependencyState; errorKind?: DependencyErrorKind }
  >>
}

interface DependencyEntry {
  state: DependencyState
  lastTransitionAt: number
  lastErrorKind?: DependencyErrorKind
  consecutiveFailures: number
  reconnectAttempts?: number
  outageStartedAt?: number
  lastSummaryAt?: number
}

const OUTAGE_SUMMARY_INTERVAL_MS = 60_000

function isDegraded(state: DependencyState): boolean {
  return state === 'reconnecting' || state === 'unhealthy'
}

export class DependencyHealthRegistry implements DependencyObserver {
  private readonly now: () => number
  private readonly logger: DependencyHealthLogger
  private readonly checks = new Map<DependencyName, DependencyCheck>()
  private readonly entries = new Map<DependencyName, DependencyEntry>()

  constructor(options?: {
    now?: () => number
    logger?: DependencyHealthLogger
  }) {
    this.now = options?.now ?? Date.now
    this.logger = options?.logger ?? ((record) => {
      console.error(JSON.stringify(record))
    })
  }

  register(name: DependencyName, check: DependencyCheck): void {
    if (this.checks.has(name)) {
      throw new Error(`dependency already registered: ${name}`)
    }
    const now = this.now()
    this.checks.set(name, check)
    this.entries.set(name, {
      state: 'starting',
      lastTransitionAt: now,
      consecutiveFailures: 0,
      ...(name === 'redisPubSub' ? { reconnectAttempts: 0 } : {}),
    })
  }

  observe(observation: DependencyObservation): void {
    this.recordAt(observation, this.now())
  }

  snapshot(): Partial<Record<DependencyName, DependencySnapshot>> {
    const snapshot: Partial<Record<DependencyName, DependencySnapshot>> = {}
    for (const [name, entry] of this.entries) {
      snapshot[name] = {
        configured: true,
        state: entry.state,
        lastTransitionAt: new Date(entry.lastTransitionAt).toISOString(),
        consecutiveFailures: entry.consecutiveFailures,
        ...(entry.lastErrorKind !== undefined
          ? { lastErrorKind: entry.lastErrorKind }
          : {}),
        ...(name === 'redisPubSub'
          ? { reconnectAttempts: entry.reconnectAttempts ?? 0 }
          : {}),
      }
    }
    return snapshot
  }

  async checkReadiness(timeoutMs = 2_000): Promise<ReadinessResult> {
    const pending = new Set(this.checks.keys())
    const checks = [...this.checks.entries()].map(([name, check]) => (
      (async () => {
        try {
          await check()
          if (pending.delete(name)) {
            this.observe({ dependency: name, state: 'healthy' })
          }
        } catch (error) {
          if (!pending.delete(name)) return
          const dependencyError = error instanceof DependencyUnavailableError
            ? error
            : findDependencyUnavailableError(error)
          this.observe({
            dependency: name,
            state: 'unhealthy',
            errorKind: dependencyError?.kind ?? 'unavailable',
          })
        }
      })()
    ))
    const group = Promise.allSettled(checks)
    let timer: ReturnType<typeof setTimeout> | undefined
    const deadline = new Promise<'timeout'>((resolve) => {
      timer = setTimeout(() => resolve('timeout'), timeoutMs)
    })

    const outcome = await Promise.race([
      group.then(() => 'settled' as const),
      deadline,
    ])
    if (outcome === 'settled') {
      if (timer !== undefined) clearTimeout(timer)
    } else {
      for (const name of pending) {
        pending.delete(name)
        this.observe({
          dependency: name,
          state: 'unhealthy',
          errorKind: 'timeout',
        })
      }
    }

    const dependencies: ReadinessResult['dependencies'] = {}
    for (const [name, entry] of this.entries) {
      dependencies[name] = {
        state: entry.state,
        ...(entry.lastErrorKind !== undefined
          ? { errorKind: entry.lastErrorKind }
          : {}),
      }
    }
    return {
      ok: Object.values(dependencies).every(
        (dependency) => dependency?.state === 'healthy',
      ),
      dependencies,
    }
  }

  private recordAt(observation: DependencyObservation, now: number): void {
    const entry = this.entries.get(observation.dependency)
    if (!entry) return

    const previous = entry.state
    const next = observation.state
    const wasDegraded = isDegraded(previous)
    const degraded = isDegraded(next)

    if (next === 'healthy') {
      entry.consecutiveFailures = 0
      delete entry.lastErrorKind
      if (observation.dependency === 'redisPubSub') {
        entry.reconnectAttempts = 0
      }
    } else {
      entry.consecutiveFailures += 1
      if (observation.errorKind !== undefined) {
        entry.lastErrorKind = observation.errorKind
      }
      if (
        observation.dependency === 'redisPubSub'
        && observation.attempt !== undefined
      ) {
        entry.reconnectAttempts = observation.attempt
      }
      if (!wasDegraded) {
        entry.outageStartedAt = now
      }
    }

    if (previous !== next) {
      entry.state = next
      entry.lastTransitionAt = now
      if (degraded) {
        entry.lastSummaryAt = now
      } else {
        delete entry.lastSummaryAt
      }
      const record: Record<string, unknown> = {
        timestamp: new Date(now).toISOString(),
        level: degraded ? 'warn' : 'info',
        event: 'dependency_state_change',
        dependency: observation.dependency,
        from: previous,
        to: next,
      }
      if (observation.attempt !== undefined) record.attempt = observation.attempt
      if (observation.nextRetryMs !== undefined) {
        record.nextRetryMs = observation.nextRetryMs
      }
      if (observation.errorKind !== undefined) {
        record.errorKind = observation.errorKind
      }
      if (!degraded && wasDegraded && entry.outageStartedAt !== undefined) {
        record.downtimeMs = Math.max(0, now - entry.outageStartedAt)
      }
      this.logger(record)
      if (!degraded) {
        delete entry.outageStartedAt
        delete entry.lastSummaryAt
      }
      return
    }

    if (
      degraded
      && now - (entry.lastSummaryAt ?? entry.lastTransitionAt)
        >= OUTAGE_SUMMARY_INTERVAL_MS
    ) {
      entry.lastSummaryAt = now
      this.logger({
        timestamp: new Date(now).toISOString(),
        level: 'warn',
        event: 'dependency_outage_summary',
        dependency: observation.dependency,
        state: next,
        consecutiveFailures: entry.consecutiveFailures,
        ...(observation.attempt !== undefined
          ? { attempt: observation.attempt }
          : {}),
        ...(observation.nextRetryMs !== undefined
          ? { nextRetryMs: observation.nextRetryMs }
          : {}),
        ...(entry.lastErrorKind !== undefined
          ? { errorKind: entry.lastErrorKind }
          : {}),
      })
    }
  }
}
