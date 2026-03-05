import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import './index.css'

async function checkAutoConnect() {
  try {
    const res = await fetch('/api/config')
    if (res.ok) {
      const config = await res.json()
      if (config.baseUrl) {
        const { useConnectionStore } = await import('./stores/connection')
        useConnectionStore.getState().setAutoConnect(config.baseUrl, config.token ?? '')
      }
    }
  } catch {
    // Not in CLI mode, ignore
  }
}

async function main() {
  await checkAutoConnect()

  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
}

main()
