import type { TaskEngine } from './engine.js'
import type { ShortTermStore } from './types.js'

export interface TaskSchedulerOptions {
  engine: TaskEngine
  shortTerm: ShortTermStore
  checkIntervalMs?: number
  pausedColdAfterMs?: number
  blockedColdAfterMs?: number
}

export class TaskScheduler {
  private engine: TaskEngine
  private shortTerm: ShortTermStore
  private checkIntervalMs: number
  private timer?: ReturnType<typeof setInterval>
  private pausedColdAfterMs?: number
  private blockedColdAfterMs?: number

  constructor(opts: TaskSchedulerOptions) {
    this.engine = opts.engine
    this.shortTerm = opts.shortTerm
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
    if (!this.shortTerm.listByStatus) return

    const blockedTasks = await this.shortTerm.listByStatus(['blocked'])
    const now = Date.now()

    for (const task of blockedTasks) {
      if (task.resumeAt && task.resumeAt <= now) {
        try {
          await this.engine.transitionTask(task.id, 'running')
        } catch {
          // Task may have been transitioned by someone else — ignore
        }
      }
    }
  }

  private async _checkColdDemotion(): Promise<void> {
    if (!this.shortTerm.listByStatus) return
    if (!this.pausedColdAfterMs && !this.blockedColdAfterMs) return

    const suspended = await this.shortTerm.listByStatus(['paused', 'blocked'])
    const now = Date.now()

    for (const task of suspended) {
      const threshold = task.status === 'paused'
        ? this.pausedColdAfterMs
        : this.blockedColdAfterMs

      if (!threshold) continue

      const age = now - task.updatedAt

      if (age >= threshold) {
        try {
          await this.engine.publishEvent(task.id, {
            type: 'taskcast:cold',
            level: 'info',
            data: {},
          })
        } catch {
          // Task may no longer be accessible — ignore
        }
      }
    }
  }
}
