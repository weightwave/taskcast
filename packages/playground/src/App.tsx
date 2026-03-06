import { TopBar } from '@/components/layout/TopBar'
import { PanelContainer } from '@/components/layout/PanelContainer'
import { BottomArea } from '@/components/layout/BottomArea'
import { useHealthCheck } from '@/hooks/useHealthCheck'

export function App() {
  useHealthCheck()
  return (
    <div className="h-screen flex flex-col bg-background text-foreground">
      <TopBar />
      <PanelContainer />
      <BottomArea />
    </div>
  )
}
