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

// ─── Storage Lifecycle ─────────────────────────────────────────────────────

export type StorageState = 'hot' | 'releasing' | 'cold'

export interface TaskStorageMetadata {
  taskId: string
  storageState: StorageState
  storageEpoch: number
  activeReleaseGeneration: string | null
  archiveWatermark: number
  lastEventAt: number | null
  coldAt: number | null
  executionDeadlineAt: number | null
  taskVersion: number
}

export interface HotWriteToken {
  taskId: string
  storageEpoch: number
}

export interface StorageLease {
  taskId: string
  lockToken: string
  generation: string
  storageEpoch: number
}

export interface TaskWriteFence {
  taskId: string
  acceptingWrites: boolean
  storageEpoch: number
  activeReleaseGeneration: string | null
}

export interface ClosedWriteFence extends TaskWriteFence {
  acceptingWrites: false
  highWatermark: number
}

export interface ReleasePreconditions {
  expectedLastEventIndex: number
  inactiveSince: number
}

export interface ReleaseResult {
  taskId: string
  storageState: StorageState
  archiveWatermark: number
  released: boolean
}

export interface ArchiveSourceManifest {
  priorWatermark: number
  targetWatermark: number
  sourceEntryCount: number
  sourceDigest: string
  seriesStateDigest: string
  expectedBatchOrdinals: number[]
}

export type ArchiveGenerationStatus = 'open' | 'finalized' | 'aborted'

export interface ArchiveGeneration {
  taskId: string
  generation: string
  storageEpoch: number
  targetWatermark: number
  manifest: ArchiveSourceManifest
  status: ArchiveGenerationStatus
  createdAt: number
  updatedAt: number
}

export interface ArchiveBatchReceipt {
  taskId: string
  generation: string
  ordinal: number
  previousBatchDigest: string | null
  batchDigest: string
  entryCount: number
  firstIndex: number | null
  lastIndex: number | null
}

export interface ArchiveSourcePage {
  taskId: string
  watermark: number
  cursor: string | null
  nextCursor: string | null
  events: TaskEvent[]
  done: boolean
}

export interface DurableSeriesState {
  taskId: string
  seriesId: string
  mode: 'latest' | 'accumulate'
  event: TaskEvent
  throughIndex: number
}

export interface RehydrateSnapshot {
  task: Task
  archiveWatermark: number
  maxEventIndex: number
  replayEvents: TaskEvent[]
  seriesLatest: DurableSeriesState[]
  storageEpoch: number
}

export interface CanonicalHistoryEntry {
  event: TaskEvent
  seriesThroughIndex?: number
}

export interface TtlClaim {
  taskId: string
  claimToken: string
  claimUntil: number
  taskVersion: number
  executionDeadlineAt: number
}

export interface TerminalProjection {
  projectionId: string
  task: Task
  event: TaskEvent
  assignment: WorkerAssignment | null
  claimToken: string | null
  claimUntil: number | null
}

export interface TaskStoragePresence {
  task: boolean
  eventCount: number
  nextIndex: boolean
  seriesStateCount: number
  writeFence: boolean
}

export interface StorageWriterRegistration {
  instanceId: string
  storageProtocolVersion: number
  build: string
  expiresAt: number
}

export interface TaskStorageMetadataCas {
  taskId: string
  expectedStorageState: StorageState
  expectedStorageEpoch: number
  expectedReleaseGeneration: string | null
  next: TaskStorageMetadata
}

export interface ArchiveBatch {
  receipt: ArchiveBatchReceipt
  events: TaskEvent[]
  seriesLatest: DurableSeriesState[]
}

export abstract class TaskStorageError extends Error {
  abstract readonly code: string
  abstract readonly retryable: boolean

  protected constructor(message: string) {
    super(message)
    this.name = new.target.name
  }
}

export class StorageFenceConflictError extends TaskStorageError {
  readonly code = 'storage_fence_conflict'
  readonly retryable = true

  constructor(message = 'Task storage write fence changed') {
    super(message)
  }
}

export class StorageBusyError extends TaskStorageError {
  readonly code = 'storage_busy'
  readonly retryable = true

  constructor(message = 'Task storage lifecycle operation is busy') {
    super(message)
  }
}

export class StorageIntegrityError extends TaskStorageError {
  readonly code = 'storage_integrity_error'
  readonly retryable = false

  constructor(message = 'Task storage integrity check failed') {
    super(message)
  }
}

export class StorageReleaseUnsupportedError extends TaskStorageError {
  readonly code = 'storage_release_unsupported'
  readonly retryable = false

  constructor(message = 'Task storage release is not supported by this adapter') {
    super(message)
  }
}

// ─── Storage Interfaces ──────────────────────────────────────────────────────

export interface BroadcastProvider {
  publish(channel: string, event: TaskEvent): Promise<void>
  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void
}

export interface ShortTermStore {
  /** True only when every lifecycle operation below is implemented atomically. */
  readonly supportsHotColdRelease?: boolean
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

  // Fenced hot/cold lifecycle. Optional only for adapters that explicitly
  // report supportsHotColdRelease !== true.
  acquireStorageLock?(
    taskId: string,
    lockToken: string,
    generation: string,
    ttlMs: number,
  ): Promise<StorageLease | null>
  renewStorageLock?(lease: StorageLease, ttlMs: number): Promise<boolean>
  releaseStorageLock?(lease: StorageLease): Promise<boolean>
  getWriteFence?(taskId: string): Promise<TaskWriteFence | null>
  closeWriteFence?(lease: StorageLease, expectedEpoch: number): Promise<ClosedWriteFence>
  reopenWriteFence?(lease: StorageLease, expectedEpoch: number): Promise<HotWriteToken>
  commitEventFenced?(
    taskId: string,
    event: Omit<TaskEvent, 'index'>,
    token: HotWriteToken,
  ): Promise<SeriesResult>
  saveTaskFenced?(task: Task, token: HotWriteToken): Promise<void>
  readArchiveSourcePage?(
    taskId: string,
    watermark: number,
    cursor: string | null,
    limit: number,
  ): Promise<ArchiveSourcePage>
  deleteTaskStorageFenced?(lease: StorageLease, expectedEpoch: number): Promise<void>
  restoreHotTaskFenced?(
    snapshot: RehydrateSnapshot,
    lease: StorageLease,
    nextEpoch: number,
  ): Promise<HotWriteToken>
  getTaskStoragePresence?(taskId: string): Promise<TaskStoragePresence>
  registerStorageWriter?(registration: StorageWriterRegistration, ttlMs: number): Promise<void>
  listStorageWriters?(): Promise<StorageWriterRegistration[]>

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
  /** True only for split-tier stores with a verifiable archive barrier. */
  readonly supportsHotColdRelease?: boolean
  /** True only when deadline claims and terminal projection are durable. */
  readonly supportsDurableTtl?: boolean
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

  // Durable lifecycle metadata and archive barrier.
  getTaskStorageMetadata?(taskId: string): Promise<TaskStorageMetadata | null>
  compareAndSetTaskStorageMetadata?(update: TaskStorageMetadataCas): Promise<boolean>
  beginArchive?(generation: ArchiveGeneration): Promise<ArchiveGeneration>
  archiveBatch?(taskId: string, generation: string, batch: ArchiveBatch): Promise<ArchiveBatchReceipt>
  finalizeArchive?(
    taskId: string,
    generation: string,
    task: Task,
    seriesLatest: DurableSeriesState[],
  ): Promise<number>
  getArchiveWatermark?(taskId: string): Promise<number>
  getLastEventIndex?(taskId: string): Promise<number>
  getRecentEvents?(taskId: string, limit: number): Promise<TaskEvent[]>
  getDurableSeriesState?(taskId: string): Promise<DurableSeriesState[]>

  // Durable execution TTL and terminal projection.
  claimOverdueTasks?(limit: number, claimTtlMs: number): Promise<TtlClaim[]>
  terminalizeTtlClaim?(
    claim: TtlClaim,
    task: Task,
    event: TaskEvent,
    assignment: WorkerAssignment | null,
  ): Promise<TerminalProjection | null>
  claimTerminalProjections?(
    limit: number,
    claimToken: string,
    claimTtlMs: number,
  ): Promise<TerminalProjection[]>
  completeTerminalProjection?(projection: TerminalProjection): Promise<void>
  saveDurableAssignment?(assignment: WorkerAssignment): Promise<void>
  deleteDurableAssignment?(taskId: string, assignmentId?: string): Promise<void>

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
