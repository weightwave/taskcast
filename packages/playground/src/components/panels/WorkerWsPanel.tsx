import { Button } from '@/components/ui/button'
import { usePanelStore } from '@/stores'
import type { Panel } from '@/stores'

export function WorkerWsPanel({ panel }: { panel: Panel }) {
  const { removePanel } = usePanelStore()

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b px-3 py-2">
        <span className="text-sm font-medium">{panel.label}</span>
        <Button
          variant="ghost"
          size="icon-xs"
          onClick={() => removePanel(panel.id)}
        >
          &times;
        </Button>
      </div>
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        Worker (WS) panel — coming soon
      </div>
    </div>
  )
}
