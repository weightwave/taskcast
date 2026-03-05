import { useConnectionStore } from '@/stores/connection'

export function getBaseUrl(): string {
  return useConnectionStore.getState().baseUrl
}

export function getAuthHeaders(): Record<string, string> {
  const jwt = useConnectionStore.getState().jwt
  if (jwt) return { Authorization: `Bearer ${jwt}` }
  return {}
}

export async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  const baseUrl = getBaseUrl()
  const headers = {
    ...getAuthHeaders(),
    ...init?.headers,
  }
  return fetch(`${baseUrl}${path}`, { ...init, headers })
}
