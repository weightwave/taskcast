import type { Task, WorkerManager, WorkerRegistration, WorkerUpdate, WorkerMatchRule, DeclineOptions } from '@taskcast/core'
import type { AuthContext } from '../auth.js'

// ─── Types ──────────────────────────────────────────────────────────────────

export interface WSLike {
  send(data: string): void
  close(): void
}

export type TaskSummary = {
  id: string
  type?: string
  tags?: string[]
  cost?: number
  params?: Record<string, unknown>
}

// ─── Helper ─────────────────────────────────────────────────────────────────

function toSummary(task: Task): TaskSummary {
  const summary: TaskSummary = { id: task.id }
  if (task.type !== undefined) summary.type = task.type
  if (task.tags !== undefined) summary.tags = task.tags
  if (task.cost !== undefined) summary.cost = task.cost
  if (task.params !== undefined) summary.params = task.params
  return summary
}

// ─── WorkerWSHandler ────────────────────────────────────────────────────────

export class WorkerWSHandler {
  private workerId: string | null = null
  private pendingOffers = new Map<string, Task>()
  private pingTimer: ReturnType<typeof setInterval> | null = null

  constructor(
    private manager: WorkerManager,
    private ws: WSLike,
    private auth?: AuthContext,
  ) {}

  // ─── Public API ─────────────────────────────────────────────────────────

  get registeredWorkerId(): string | null {
    return this.workerId
  }

  async handleMessage(raw: string): Promise<void> {
    let msg: Record<string, unknown>
    try {
      msg = JSON.parse(raw) as Record<string, unknown>
    } catch {
      this.send({ type: 'error', message: 'Invalid JSON' })
      return
    }

    const type = msg.type as string | undefined
    switch (type) {
      case 'register':
        await this.handleRegister(msg)
        break
      case 'update':
        await this.handleUpdate(msg)
        break
      case 'accept':
        await this.handleAccept(msg)
        break
      case 'decline':
        await this.handleDecline(msg)
        break
      case 'claim':
        await this.handleClaim(msg)
        break
      case 'drain':
        await this.handleDrain()
        break
      case 'pong':
        await this.handlePong()
        break
      default:
        this.send({ type: 'error', message: `Unknown message type: ${String(type)}` })
        break
    }
  }

  offerTask(task: Task): void {
    this.pendingOffers.set(task.id, task)
    this.send({ type: 'offer', taskId: task.id, task: toSummary(task) })
  }

  broadcastAvailable(task: Task): void {
    this.send({ type: 'available', taskId: task.id, task: toSummary(task) })
  }

  async handleDisconnect(): Promise<void> {
    this.stopPingTimer()
    if (this.workerId) {
      await this.manager.unregisterWorker(this.workerId)
    }
  }

  stopPingTimer(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer)
      this.pingTimer = null
    }
  }

  // ─── Message Handlers ──────────────────────────────────────────────────

  private async handleRegister(msg: Record<string, unknown>): Promise<void> {
    if (typeof msg.workerId === 'string' && this.auth?.workerId && this.auth.workerId !== msg.workerId) {
      this.send({ type: 'error', message: 'Forbidden: worker ID mismatch', code: 'FORBIDDEN' })
      return
    }
    const reg: WorkerRegistration = {
      matchRule: (msg.matchRule as Record<string, unknown> | undefined) ?? {},
      capacity: (msg.capacity as number | undefined) ?? 10,
      connectionMode: 'websocket',
    }
    if (typeof msg.workerId === 'string') reg.id = msg.workerId
    if (typeof msg.weight === 'number') reg.weight = msg.weight
    const worker = await this.manager.registerWorker(reg)
    this.workerId = worker.id
    this.send({ type: 'registered', workerId: worker.id })
    this.startPingTimer()
  }

  private async handleUpdate(msg: Record<string, unknown>): Promise<void> {
    if (!this.requireRegistered()) return
    const update: WorkerUpdate = {}
    if (typeof msg.weight === 'number') update.weight = msg.weight
    if (typeof msg.capacity === 'number') update.capacity = msg.capacity
    if (msg.matchRule !== undefined) update.matchRule = msg.matchRule as WorkerMatchRule
    await this.manager.updateWorker(this.workerId!, update)
  }

  private async handleAccept(msg: Record<string, unknown>): Promise<void> {
    if (!this.requireRegistered()) return
    if (typeof msg.taskId !== 'string') {
      this.send({ type: 'error', message: 'taskId is required and must be a string', code: 'INVALID_MESSAGE' })
      return
    }
    const taskId = msg.taskId as string
    const result = await this.manager.claimTask(taskId, this.workerId!)
    if (result.success) {
      this.pendingOffers.delete(taskId)
      this.send({ type: 'assigned', taskId })
    } else {
      this.send({ type: 'error', message: result.reason!, taskId })
    }
  }

  private async handleDecline(msg: Record<string, unknown>): Promise<void> {
    if (!this.requireRegistered()) return
    if (typeof msg.taskId !== 'string') {
      this.send({ type: 'error', message: 'taskId is required and must be a string', code: 'INVALID_MESSAGE' })
      return
    }
    const taskId = msg.taskId as string
    this.pendingOffers.delete(taskId)
    const opts: DeclineOptions = {}
    if (typeof msg.blacklist === 'boolean') opts.blacklist = msg.blacklist
    await this.manager.declineTask(taskId, this.workerId!, opts)
    this.send({ type: 'declined', taskId })
  }

  private async handleClaim(msg: Record<string, unknown>): Promise<void> {
    if (!this.requireRegistered()) return
    if (typeof msg.taskId !== 'string') {
      this.send({ type: 'error', message: 'taskId is required and must be a string', code: 'INVALID_MESSAGE' })
      return
    }
    const taskId = msg.taskId as string
    const result = await this.manager.claimTask(taskId, this.workerId!)
    this.send({ type: 'claimed', taskId, success: result.success })
  }

  private async handleDrain(): Promise<void> {
    if (!this.requireRegistered()) return
    await this.manager.updateWorker(this.workerId!, { status: 'draining' })
  }

  private async handlePong(): Promise<void> {
    if (!this.requireRegistered()) return
    await this.manager.heartbeat(this.workerId!)
  }

  // ─── Helpers ──────────────────────────────────────────────────────────

  private startPingTimer(): void {
    this.stopPingTimer()
    const intervalMs = this.manager.heartbeatIntervalMs
    this.pingTimer = setInterval(() => {
      this.send({ type: 'ping' })
    }, intervalMs)
  }

  private requireRegistered(): boolean {
    if (!this.workerId) {
      this.send({ type: 'error', message: 'Not registered' })
      return false
    }
    return true
  }

  private send(data: Record<string, unknown>): void {
    this.ws.send(JSON.stringify(data))
  }
}
