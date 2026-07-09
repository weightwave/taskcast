// ─── Task ───────────────────────────────────────────────────────────────────

export type TaskStatus =
  | 'pending'
  | 'assigned'
  | 'running'
  | 'paused'
  | 'blocked'
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
  | 'worker:connect'
  | 'worker:manage'
  | 'task:resolve'
  | 'task:signal'
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

export interface BlockedRequest {
  type: string
  data: unknown
}

// ─── Worker Assignment ──────────────────────────────────────────────────────

export type AssignMode = 'external' | 'pull' | 'ws-offer' | 'ws-race'

export type DisconnectPolicy = 'reassign' | 'mark' | 'fail'

export type WorkerStatus = 'idle' | 'busy' | 'draining' | 'offline'

export interface TagMatcher {
  all?: string[]
  any?: string[]
  none?: string[]
}

export interface WorkerMatchRule {
  taskTypes?: string[]
  tags?: TagMatcher
}

export interface Worker {
  id: string
  status: WorkerStatus
  matchRule: WorkerMatchRule
  capacity: number
  usedSlots: number
  weight: number
  connectionMode: 'pull' | 'websocket'
  connectedAt: number
  lastHeartbeatAt: number
  metadata?: Record<string, unknown>
}

export type WorkerAssignmentStatus = 'offered' | 'assigned' | 'running'

export interface WorkerAssignment {
  taskId: string
  workerId: string
  cost: number
  assignedAt: number
  status: WorkerAssignmentStatus
}

export interface WorkerAuditEvent {
  id: string
  workerId: string
  timestamp: number
  action:
    | 'connected'
    | 'disconnected'
    | 'updated'
    | 'task_assigned'
    | 'task_declined'
    | 'task_reclaimed'
    | 'draining'
    | 'heartbeat_timeout'
    | 'pull_request'
  data?: Record<string, unknown>
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
  tags?: string[]
  assignMode?: AssignMode
  cost?: number
  assignedWorker?: string
  reason?: string
  resumeAt?: number
  blockedRequest?: BlockedRequest
  disconnectPolicy?: DisconnectPolicy
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
  seriesAccField?: string
  seriesSnapshot?: boolean
  /** Transient: accumulated data attached during broadcast, not persisted in ShortTermStore */
  _accumulatedData?: unknown
}

/**
 * Archive-persistable event shape.
 *
 * TaskArchive v1 stores a compacted, replayable event stream for one task:
 * indexes must be contiguous from 0, latest-mode histories are latest-only,
 * and accumulate-mode histories may be stored as accumulated snapshots.
 * Presentation/transient event fields such as collapsed `seriesSnapshot` events
 * and broadcast `_accumulatedData` are not valid archive data.
 */
export type TaskArchiveEvent = Omit<TaskEvent, 'seriesSnapshot' | '_accumulatedData'>

export interface TaskArchive {
  schema: 'taskcast.taskArchive'
  version: 1
  exportedAt: number
  task: Task
  /** Compacted, replayable event stream for the task, ordered by contiguous indexes from 0. */
  events: TaskArchiveEvent[]
}

export interface TaskArchiveImportOptions {
  overwrite?: boolean
}

export interface TaskArchiveImportResult {
  taskId: string
  eventCount: number
  overwritten: boolean
}

export interface SeriesLatestEntry {
  taskId: string
  seriesId: string
  event: TaskArchiveEvent
}

export interface TaskArchiveRestoreData {
  task: Task
  events: TaskArchiveEvent[]
  nextIndex: number
  seriesLatest: SeriesLatestEntry[]
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
  seriesAccField?: string
  seriesSnapshot?: boolean
}

// ─── Subscription ────────────────────────────────────────────────────────────

export interface SinceCursor {
  id?: string
  index?: number
  timestamp?: number
}

export type SeriesFormat = 'delta' | 'accumulated'

export interface SubscribeFilter {
  since?: SinceCursor
  types?: string[]
  levels?: Level[]
  includeStatus?: boolean
  wrap?: boolean
  seriesFormat?: SeriesFormat
}

export interface EventQueryOptions {
  since?: SinceCursor
  limit?: number
}

export interface SeriesResult {
  /** The original delta event (stored in ShortTermStore) */
  event: TaskEvent
  /** The event with accumulated data (for LongTermStore + broadcast). Undefined for non-accumulate modes. */
  accumulatedEvent?: TaskEvent
  /** Whether processSeries already stored the event (e.g. latest mode uses replaceLastSeriesEvent). */
  stored?: boolean
}

// ─── Storage Interfaces ──────────────────────────────────────────────────────

export interface BroadcastProvider {
  publish(channel: string, event: TaskEvent): Promise<void>
  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void
}

export interface ShortTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  /** Atomically allocates the next event index for a task. */
  nextIndex(taskId: string): Promise<number>
  appendEvent(taskId: string, event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]>
  setTTL(taskId: string, ttlSeconds: number): Promise<void>
  getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null>
  setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
  /** Atomically read previous accumulated value, concatenate with new delta, write back. Returns the accumulated event. */
  accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent>
  replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
  /** Validates deterministic archive restore conflicts before mutation; engine calls this before multi-store restore. */
  validateTaskArchiveRestore?(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<void>
  /** Stores with native archive restore should implement this; engine import checks availability before use. */
  restoreTaskArchive?(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }>

  // Task query
  listTasks(filter: TaskFilter): Promise<Task[]>

  // Worker state
  saveWorker(worker: Worker): Promise<void>
  getWorker(workerId: string): Promise<Worker | null>
  listWorkers(filter?: WorkerFilter): Promise<Worker[]>
  deleteWorker(workerId: string): Promise<void>

  // Atomic claim
  claimTask(taskId: string, workerId: string, cost: number): Promise<boolean>

  // Worker assignments
  addAssignment(assignment: WorkerAssignment): Promise<void>
  removeAssignment(taskId: string): Promise<void>
  getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]>
  getTaskAssignment(taskId: string): Promise<WorkerAssignment | null>

  // TTL management
  clearTTL(taskId: string): Promise<void>

  // Task query by status
  listByStatus(statuses: TaskStatus[]): Promise<Task[]>
}

export interface LongTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  saveEvent(event: TaskEvent): Promise<void>
  /** Optional series-aware durable write for latest-mode series. */
  replaceLastSeriesEvent?(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
  /** Optional series-aware durable write for accumulate-mode series. Returns the accumulated event. */
  accumulateSeries?(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent>
  getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]>
  /**
   * True when short-term archive restore writes the same durable storage this
   * long-term store reads from. The engine still runs long-term preflight, but
   * skips a duplicate long-term final restore.
   */
  sharesTaskArchiveRestoreStorage?: boolean
  /** Validates deterministic archive restore conflicts before mutation; engine calls this before multi-store restore. */
  validateTaskArchiveRestore?(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<void>
  /** Stores with native archive restore should implement this; engine import checks availability before use. */
  restoreTaskArchive?(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }>
  saveWorkerEvent(event: WorkerAuditEvent): Promise<void>
  getWorkerEvents(workerId: string, opts?: EventQueryOptions): Promise<WorkerAuditEvent[]>
}

export interface TaskFilter {
  status?: TaskStatus[]
  types?: string[]
  tags?: TagMatcher
  assignMode?: AssignMode[]
  excludeTaskIds?: string[]
  limit?: number
}

export interface WorkerFilter {
  status?: WorkerStatus[]
  connectionMode?: ('pull' | 'websocket')[]
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
  onTaskCreated?(task: Task): void
  onTaskTransitioned?(task: Task, from: TaskStatus, to: TaskStatus): void
  onWorkerConnected?(worker: Worker): void
  onWorkerDisconnected?(worker: Worker, reason: string): void
  onTaskAssigned?(task: Task, worker: Worker): void
  onTaskDeclined?(task: Task, worker: Worker, blacklisted: boolean): void
}
