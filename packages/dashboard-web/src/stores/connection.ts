import { create } from 'zustand'
import { persist } from 'zustand/middleware'

interface ConnectionState {
  baseUrl: string
  jwt: string | null
  connected: boolean
  error: string | null
  connect: (url: string, adminToken: string) => Promise<void>
  disconnect: () => void
  setAutoConnect: (baseUrl: string, jwt: string) => void
}

export const useConnectionStore = create<ConnectionState>()(
  persist(
    (set) => ({
      baseUrl: '',
      jwt: null,
      connected: false,
      error: null,

      connect: async (url: string, adminToken: string) => {
        try {
          set({ error: null })

          // 1. Health check
          const healthRes = await fetch(`${url}/health`)
          if (!healthRes.ok) throw new Error('Server unreachable')

          // 2. Exchange admin token for JWT
          const tokenRes = await fetch(`${url}/admin/token`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ adminToken }),
          })

          if (tokenRes.status === 401) throw new Error('Invalid admin token')
          if (tokenRes.status === 404) {
            // Admin API not enabled — try connecting without JWT
            // (server might be in auth: none mode)
            set({ baseUrl: url, jwt: null, connected: true, error: null })
            return
          }
          if (!tokenRes.ok) throw new Error(`Token exchange failed: ${tokenRes.status}`)

          const { token: jwt } = await tokenRes.json()
          set({ baseUrl: url, jwt: jwt || null, connected: true, error: null })
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err)
          set({ connected: false, error: message })
          throw err
        }
      },

      disconnect: () => {
        set({ jwt: null, connected: false, error: null })
      },

      setAutoConnect: (baseUrl: string, jwt: string) => {
        set({ baseUrl, jwt: jwt || null, connected: true, error: null })
      },
    }),
    {
      name: 'taskcast-dashboard-connection',
      partialize: (state) => ({ baseUrl: state.baseUrl }),
    },
  ),
)
