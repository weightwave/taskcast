import type { Task, Worker } from '@taskcast/core'

/**
 * Extended Task type returned by the dashboard API.
 * Includes runtime-enriched fields beyond the core Task definition.
 */
export interface DashboardTask extends Task {
  /** Whether the task has active SSE subscribers (hot) or not (cold). */
  hot?: boolean
  /** Number of active SSE subscribers. */
  subscriberCount?: number
  /** Alias for assignedWorker used in some API responses. */
  workerId?: string
}

/** Re-export core Worker type (no dashboard-specific extensions needed currently). */
export type { Task, Worker }
