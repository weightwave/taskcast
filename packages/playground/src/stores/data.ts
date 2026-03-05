import { create } from 'zustand'
import type { Task, TaskEvent } from '@taskcast/core'

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
  globalEvents: TaskEvent[]
  webhookLogs: WebhookLog[]
  setTasks: (tasks: Task[]) => void
  addTask: (task: Task) => void
  updateTask: (task: Task) => void
  addEvent: (event: TaskEvent) => void
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
  addEvent: (event) =>
    set((state) => ({
      globalEvents: [event, ...state.globalEvents].slice(0, 500),
    })),
  addWebhookLog: (log) =>
    set((state) => ({
      webhookLogs: [log, ...state.webhookLogs].slice(0, 200),
    })),
  clearAll: () => set({ tasks: [], globalEvents: [], webhookLogs: [] }),
}))
