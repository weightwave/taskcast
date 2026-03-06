import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'

export function WorkerSummary({
  onlineWorkers,
  totalCapacity,
  usedCapacity,
}: {
  onlineWorkers: number
  totalCapacity: number
  usedCapacity: number
}) {
  const utilizationPercent = totalCapacity > 0 ? Math.round((usedCapacity / totalCapacity) * 100) : 0

  return (
    <Card>
      <CardHeader>
        <CardTitle>Workers</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex items-center justify-between">
          <span className="text-sm text-muted-foreground">Online Workers</span>
          <span className="text-2xl font-bold">{onlineWorkers}</span>
        </div>
        <div className="space-y-2">
          <div className="flex items-center justify-between text-sm">
            <span className="text-muted-foreground">Capacity</span>
            <span>
              {usedCapacity} / {totalCapacity} slots ({utilizationPercent}%)
            </span>
          </div>
          <Progress value={utilizationPercent} />
        </div>
      </CardContent>
    </Card>
  )
}
