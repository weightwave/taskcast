import { create } from 'zustand'

export interface ConnectionState {
  mode: 'embedded' | 'external'
  baseUrl: string
  token: string
  connected: boolean
  setMode: (mode: 'embedded' | 'external') => void
  setBaseUrl: (url: string) => void
  setToken: (token: string) => void
  setConnected: (connected: boolean) => void
}

export const useConnectionStore = create<ConnectionState>((set) => ({
  mode: 'embedded',
  baseUrl: '/taskcast',
  token: '',
  connected: false,
  setMode: (mode) => set({ mode, baseUrl: mode === 'embedded' ? '/taskcast' : '' }),
  setBaseUrl: (baseUrl) => set({ baseUrl }),
  setToken: (token) => set({ token }),
  setConnected: (connected) => set({ connected }),
}))
