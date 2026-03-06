import { create } from 'zustand'
import type { Task, TaskEvent } from '@taskcast/core'

export type EventDirection = 'sent' | 'received'

export interface GlobalEvent extends TaskEvent {
  direction?: EventDirection
  sourceLabel?: string
}

export interface WebhookLog {
  id: string
  timestamp: number
  url: string
  payload: unknown
  statusCode?: number
  error?: string
}

interface DataState {
  tasks: Task[]
  globalEvents: GlobalEvent[]
  webhookLogs: WebhookLog[]
  /** O(1) dedup index keyed by `${event.id}:${direction}:${sourceLabel}` */
  _eventKeys: Set<string>
  setTasks: (tasks: Task[]) => void
  addTask: (task: Task) => void
  updateTask: (task: Task) => void
  addEvent: (event: TaskEvent, direction?: EventDirection, sourceLabel?: string) => void
  addWebhookLog: (log: WebhookLog) => void
  clearAll: () => void
}

export const useDataStore = create<DataState>((set) => ({
  tasks: [],
  globalEvents: [],
  webhookLogs: [],
  _eventKeys: new Set<string>(),
  setTasks: (tasks) => set({ tasks }),
  addTask: (task) => set((state) => ({ tasks: [...state.tasks, task] })),
  updateTask: (task) =>
    set((state) => ({
      tasks: state.tasks.map((t) => (t.id === task.id ? task : t)),
    })),
  addEvent: (event, direction, sourceLabel) =>
    set((state) => {
      const key = `${event.id}:${direction ?? ''}:${sourceLabel ?? ''}`
      if (state._eventKeys.has(key)) {
        return state
      }
      const entry: GlobalEvent = { ...event, direction, sourceLabel }
      const list = [entry, ...state.globalEvents]
      list.sort((a, b) => b.timestamp - a.timestamp)
      const trimmed = list.slice(0, 500)
      const newKeys = new Set(state._eventKeys)
      newKeys.add(key)
      return { globalEvents: trimmed, _eventKeys: newKeys }
    }),
  addWebhookLog: (log) =>
    set((state) => ({
      webhookLogs: [log, ...state.webhookLogs].slice(0, 200),
    })),
  clearAll: () => set({ tasks: [], globalEvents: [], webhookLogs: [], _eventKeys: new Set<string>() }),
}))
