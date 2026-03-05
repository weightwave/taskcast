import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Separator } from '@/components/ui/separator'
import { formatRelativeTime, cn } from '@/lib/utils'
import { workerStatusColor } from '@/lib/status'
import type { Worker } from '@taskcast/core'

export function WorkerDetail({ worker }: { worker: Worker }) {
  const w = worker

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center gap-3 flex-wrap">
        <h3 className="text-lg font-semibold font-mono">{w.id}</h3>
        <Badge variant="outline" className={cn(workerStatusColor(w.status))}>
          {w.status}
        </Badge>
      </div>

      {/* Connection info */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Connection Info</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2 text-sm">
          <InfoRow label="ID" value={w.id} mono />
          <InfoRow label="Mode" value={w.connectionMode ?? '-'} />
          <InfoRow label="Weight" value={w.weight != null ? String(w.weight) : '-'} />
          <InfoRow label="Capacity" value={`${w.usedSlots} / ${w.capacity} slots`} />
          {w.connectedAt && <InfoRow label="Connected" value={formatRelativeTime(w.connectedAt)} />}
          {w.lastHeartbeatAt && <InfoRow label="Last Heartbeat" value={formatRelativeTime(w.lastHeartbeatAt)} />}
        </CardContent>
      </Card>

      {/* Match Rule */}
      {w.matchRule != null && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Match Rule</CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="max-h-48 overflow-auto rounded bg-muted p-3 text-xs">
              {JSON.stringify(w.matchRule, null, 2)}
            </pre>
          </CardContent>
        </Card>
      )}

      {/* Metadata */}
      {w.metadata != null && Object.keys(w.metadata).length > 0 && (
        <>
          <Separator />
          <Card>
            <CardHeader>
              <CardTitle className="text-sm">Metadata</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              {Object.entries(w.metadata).map(([key, value]) => (
                <InfoRow
                  key={key}
                  label={key}
                  value={typeof value === 'string' ? value : JSON.stringify(value)}
                />
              ))}
            </CardContent>
          </Card>
        </>
      )}
    </div>
  )
}

function InfoRow({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex justify-between gap-4">
      <span className="text-muted-foreground shrink-0">{label}</span>
      <span className={cn('truncate text-right', mono && 'font-mono text-xs')}>{value}</span>
    </div>
  )
}
