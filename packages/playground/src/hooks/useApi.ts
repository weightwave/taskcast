import { useCallback } from 'react'
import { useConnectionStore } from '@/stores'
import type { Panel } from '@/stores'

export function useApi(panel: Panel) {
  const { baseUrl, token } = useConnectionStore()

  const effectiveToken =
    panel.useAuth === 'none' ? undefined : panel.useAuth === 'custom' ? panel.customToken : token

  const headers = useCallback(
    (extra?: Record<string, string>): Record<string, string> => {
      const h: Record<string, string> = { 'Content-Type': 'application/json', ...extra }
      if (effectiveToken) h['Authorization'] = `Bearer ${effectiveToken}`
      return h
    },
    [effectiveToken],
  )

  const apiFetch = useCallback(
    async (path: string, init?: RequestInit) => {
      const url = `${baseUrl}${path}`
      return fetch(url, {
        ...init,
        headers: { ...headers(), ...(init?.headers as Record<string, string>) },
      })
    },
    [baseUrl, headers],
  )

  return { baseUrl, effectiveToken, headers, apiFetch }
}
