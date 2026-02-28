// ─── Task ───────────────────────────────────────────────────────────────────

export type TaskStatus =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'timeout'
  | 'cancelled'

export interface TaskError {
  code?: string
  message: string
  details?: Record<string, unknown>
}

export interface TaskAuthConfig {
  rules: Array<{
    match: { scope: PermissionScope[] }
    require: {
      claims?: Record<string, unknown>
      sub?: string[]
    }
  }>
}

export interface WebhookConfig {
  url: string
  filter?: SubscribeFilter
  secret?: string
  wrap?: boolean
  retry?: RetryConfig
}

export interface RetryConfig {
  retries: number
  backoff: 'fixed' | 'exponential' | 'linear'
  initialDelayMs: number
  maxDelayMs: number
  timeoutMs: number
}

export type SeriesMode = 'keep-all' | 'accumulate' | 'latest'

export type Level = 'debug' | 'info' | 'warn' | 'error'

export type PermissionScope =
  | 'task:create'
  | 'task:manage'
  | 'event:publish'
  | 'event:subscribe'
  | 'event:history'
  | 'webhook:create'
  | '*'

export interface CleanupRule {
  name?: string
  match?: {
    taskTypes?: string[]
    status?: TaskStatus[]
  }
  trigger: {
    afterMs?: number
  }
  target: 'all' | 'events' | 'task'
  eventFilter?: {
    types?: string[]
    levels?: Level[]
    olderThanMs?: number
    seriesMode?: SeriesMode[]
  }
}

export interface Task {
  id: string
  type?: string
  status: TaskStatus
  params?: Record<string, unknown>
  result?: Record<string, unknown>
  error?: TaskError
  metadata?: Record<string, unknown>
  createdAt: number
  updatedAt: number
  completedAt?: number
  ttl?: number
  authConfig?: TaskAuthConfig
  webhooks?: WebhookConfig[]
  cleanup?: { rules: CleanupRule[] }
}

// ─── Events ─────────────────────────────────────────────────────────────────

export interface TaskEvent {
  id: string
  taskId: string
  index: number
  timestamp: number
  type: string
  level: Level
  data: unknown
  seriesId?: string
  seriesMode?: SeriesMode
}

export interface SSEEnvelope {
  filteredIndex: number
  rawIndex: number
  eventId: string
  taskId: string
  type: string
  timestamp: number
  level: Level
  data: unknown
  seriesId?: string
  seriesMode?: SeriesMode
}

// ─── Subscription ────────────────────────────────────────────────────────────

export interface SinceCursor {
  id?: string
  index?: number
  timestamp?: number
}

export interface SubscribeFilter {
  since?: SinceCursor
  types?: string[]
  levels?: Level[]
  includeStatus?: boolean
  wrap?: boolean
}

export interface EventQueryOptions {
  since?: SinceCursor
  limit?: number
}

// ─── Storage Interfaces ──────────────────────────────────────────────────────

export interface BroadcastProvider {
  publish(channel: string, event: TaskEvent): Promise<void>
  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void
}

export interface ShortTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  appendEvent(taskId: string, event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]>
  setTTL(taskId: string, ttlSeconds: number): Promise<void>
  getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null>
  setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
  replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
}

export interface LongTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  saveEvent(event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]>
}

// ─── Hooks ───────────────────────────────────────────────────────────────────

export interface ErrorContext {
  operation: string
  taskId?: string
}

export interface TaskcastHooks {
  onTaskFailed?(task: Task, error: TaskError): void
  onTaskTimeout?(task: Task): void
  onUnhandledError?(err: unknown, context: ErrorContext): void
  onEventDropped?(event: TaskEvent, reason: string): void
  onWebhookFailed?(config: WebhookConfig, err: unknown): void
  onSSEConnect?(taskId: string, clientId: string): void
  onSSEDisconnect?(taskId: string, clientId: string, duration: number): void
}
