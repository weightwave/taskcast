import { create } from 'zustand'

export type PanelType = 'backend' | 'browser' | 'worker-pull' | 'worker-ws'

export interface Panel {
  id: string
  type: PanelType
  label: string
  customToken?: string
  useAuth: 'global' | 'custom' | 'none'
}

interface PanelState {
  panels: Panel[]
  addPanel: (type: PanelType) => void
  removePanel: (id: string) => void
  updatePanel: (id: string, update: Partial<Panel>) => void
}

let counter = 0
const labelMap: Record<PanelType, string> = {
  backend: 'Backend',
  browser: 'Browser',
  'worker-pull': 'Worker (Pull)',
  'worker-ws': 'Worker (WS)',
}

export const usePanelStore = create<PanelState>((set) => ({
  panels: [],
  addPanel: (type) =>
    set((state) => {
      counter++
      const typeCount = state.panels.filter((p) => p.type === type).length + 1
      return {
        panels: [
          ...state.panels,
          {
            id: `panel-${counter}`,
            type,
            label: `${labelMap[type]} ${typeCount}`,
            useAuth: 'global',
          },
        ],
      }
    }),
  removePanel: (id) => set((state) => ({ panels: state.panels.filter((p) => p.id !== id) })),
  updatePanel: (id, update) =>
    set((state) => ({
      panels: state.panels.map((p) => (p.id === id ? { ...p, ...update } : p)),
    })),
}))
