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
  setTasks: (tasks) => set({ tasks }),
  addTask: (task) => set((state) => ({ tasks: [...state.tasks, task] })),
  updateTask: (task) =>
    set((state) => ({
      tasks: state.tasks.map((t) => (t.id === task.id ? task : t)),
    })),
  addEvent: (event, direction, sourceLabel) =>
    set((state) => {
      const key = `${event.id}:${sourceLabel ?? ''}`
      if (state.globalEvents.some((e) => `${e.id}:${e.sourceLabel ?? ''}` === key)) {
        return state
      }
      const entry: GlobalEvent = { ...event, direction, sourceLabel }
      const list = [entry, ...state.globalEvents]
      list.sort((a, b) => b.timestamp - a.timestamp)
      return { globalEvents: list.slice(0, 500) }
    }),
  addWebhookLog: (log) =>
    set((state) => ({
      webhookLogs: [log, ...state.webhookLogs].slice(0, 200),
    })),
  clearAll: () => set({ tasks: [], globalEvents: [], webhookLogs: [] }),
}))
