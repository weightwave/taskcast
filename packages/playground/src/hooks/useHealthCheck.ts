import { useEffect } from 'react'
import { useConnectionStore } from '@/stores'

export function useHealthCheck() {
  const { baseUrl, setConnected } = useConnectionStore()

  useEffect(() => {
    let cancelled = false

    async function check() {
      try {
        const res = await fetch(`${baseUrl}/health`)
        if (!cancelled) setConnected(res.ok)
      } catch {
        if (!cancelled) setConnected(false)
      }
    }

    check()
    const interval = setInterval(check, 5000)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [baseUrl, setConnected])
}
