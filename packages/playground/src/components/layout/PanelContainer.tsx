import type { ReactNode } from 'react'
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from '@/components/ui/resizable'
import { usePanelStore } from '@/stores'
import type { Panel } from '@/stores'
import { BackendPanel } from '@/components/panels/BackendPanel'
import { BrowserPanel } from '@/components/panels/BrowserPanel'
import { WorkerPullPanel } from '@/components/panels/WorkerPullPanel'
import { WorkerWsPanel } from '@/components/panels/WorkerWsPanel'

function PanelRenderer({ panel }: { panel: Panel }) {
  switch (panel.type) {
    case 'backend':
      return <BackendPanel panel={panel} />
    case 'browser':
      return <BrowserPanel panel={panel} />
    case 'worker-pull':
      return <WorkerPullPanel panel={panel} />
    case 'worker-ws':
      return <WorkerWsPanel panel={panel} />
  }
}

export function PanelContainer() {
  const { panels } = usePanelStore()

  if (panels.length === 0) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        Click '+ Add Role' to get started
      </div>
    )
  }

  const elements: ReactNode[] = []
  panels.forEach((panel, i) => {
    if (i > 0) {
      elements.push(<ResizableHandle key={`handle-${panel.id}`} withHandle />)
    }
    elements.push(
      <ResizablePanel key={panel.id} minSize={15}>
        <PanelRenderer panel={panel} />
      </ResizablePanel>,
    )
  })

  return (
    <ResizablePanelGroup direction="horizontal" className="flex-1">
      {elements}
    </ResizablePanelGroup>
  )
}
