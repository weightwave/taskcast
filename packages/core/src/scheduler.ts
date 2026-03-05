import type { TaskEngine } from './engine.js'
import type { ShortTermStore } from './types.js'

export interface TaskSchedulerOptions {
  engine: TaskEngine
  shortTermStore: ShortTermStore
  checkIntervalMs?: number        // default 60_000
  pausedColdAfterMs?: number      // default undefined (disabled)
  blockedColdAfterMs?: number     // default undefined (disabled)
}

export class TaskScheduler {
  private engine: TaskEngine
  private shortTermStore: ShortTermStore
  private checkIntervalMs: number
  private timer?: ReturnType<typeof setInterval>
  private pausedColdAfterMs?: number
  private blockedColdAfterMs?: number

  constructor(opts: TaskSchedulerOptions) {
    this.engine = opts.engine
    this.shortTermStore = opts.shortTermStore
    this.checkIntervalMs = opts.checkIntervalMs ?? 60_000
    if (opts.pausedColdAfterMs !== undefined) this.pausedColdAfterMs = opts.pausedColdAfterMs
    if (opts.blockedColdAfterMs !== undefined) this.blockedColdAfterMs = opts.blockedColdAfterMs
  }

  start(): void {
    this.timer = setInterval(() => this.tick().catch(() => {}), this.checkIntervalMs)
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer)
  }

  async tick(): Promise<void> {
    await this._checkWakeUpTimers()
    await this._checkColdDemotion()
  }

  private async _checkWakeUpTimers(): Promise<void> {
    const blockedTasks = await this.shortTermStore.listByStatus(['blocked'])
    const now = Date.now()
    for (const task of blockedTasks) {
      if (task.resumeAt && task.resumeAt <= now) {
        try {
          await this.engine.transitionTask(task.id, 'running')
        } catch {
          // Task may have been transitioned by someone else
        }
      }
    }
  }

  private async _checkColdDemotion(): Promise<void> {
    if (!this.pausedColdAfterMs && !this.blockedColdAfterMs) return
    const suspended = await this.shortTermStore.listByStatus(['paused', 'blocked'])
    const now = Date.now()
    for (const task of suspended) {
      const threshold = task.status === 'paused' ? this.pausedColdAfterMs : this.blockedColdAfterMs
      if (!threshold) continue
      if (now - task.updatedAt >= threshold) {
        try {
          await this.engine.publishEvent(task.id, {
            type: 'taskcast:cold',
            level: 'info',
            data: {},
          })
        } catch { /* best-effort */ }
      }
    }
  }
}
