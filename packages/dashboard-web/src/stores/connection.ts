import { create } from 'zustand'

interface ConnectionState {
  baseUrl: string
  jwt: string | null
  connected: boolean
  error: string | null
  connect: (url: string, adminToken: string) => Promise<void>
  disconnect: () => void
}

export const useConnectionStore = create<ConnectionState>()((set) => ({
  baseUrl: '',
  jwt: null,
  connected: false,
  error: null,
  connect: async () => {},
  disconnect: () => set({ jwt: null, connected: false }),
}))
