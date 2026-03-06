import { cn } from '@/lib/utils'
import { useConnectionStore } from '@/stores/connection'

export function Header() {
  const { baseUrl, connected, disconnect } = useConnectionStore()

  return (
    <header className="flex items-center justify-between border-b px-6 py-3">
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">Server:</span>
        <code className="text-sm">{baseUrl}</code>
        <span
          className={cn(
            'inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium',
            connected
              ? 'bg-green-100 text-green-800'
              : 'bg-red-100 text-red-800',
          )}
        >
          {connected ? 'Connected' : 'Disconnected'}
        </span>
      </div>
      {connected && (
        <button
          onClick={disconnect}
          className="rounded-md border px-3 py-1.5 text-sm hover:bg-muted"
        >
          Disconnect
        </button>
      )}
    </header>
  )
}
