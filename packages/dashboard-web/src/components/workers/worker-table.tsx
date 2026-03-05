import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { formatRelativeTime, cn } from '@/lib/utils'
import { workerStatusColor } from '@/lib/status'
import { useDrainWorker, useDisconnectWorker } from '@/hooks/use-workers'
import type { Worker } from '@taskcast/core'

export function WorkerTable({
  workers,
  selectedWorkerId,
  onSelect,
}: {
  workers: Worker[]
  selectedWorkerId: string | null
  onSelect: (workerId: string) => void
}) {
  const drain = useDrainWorker()
  const disconnect = useDisconnectWorker()
  const [disconnectTarget, setDisconnectTarget] = useState<string | null>(null)

  if (workers.length === 0) {
    return (
      <div className="flex h-40 items-center justify-center text-sm text-muted-foreground">
        No workers connected.
      </div>
    )
  }

  function handleDrainToggle(worker: Worker) {
    const newStatus = worker.status === 'draining' ? 'idle' : 'draining'
    drain.mutate({ workerId: worker.id, status: newStatus })
  }

  function confirmDisconnect() {
    if (disconnectTarget) {
      disconnect.mutate(disconnectTarget, {
        onSettled: () => setDisconnectTarget(null),
      })
    }
  }

  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>ID</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Capacity</TableHead>
            <TableHead>Mode</TableHead>
            <TableHead>Weight</TableHead>
            <TableHead>Last Heartbeat</TableHead>
            <TableHead>Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {workers.map((worker) => {
            const utilization = worker.capacity > 0 ? Math.round((worker.usedSlots / worker.capacity) * 100) : 0
            return (
              <TableRow
                key={worker.id}
                className="cursor-pointer"
                data-state={worker.id === selectedWorkerId ? 'selected' : undefined}
                onClick={() => onSelect(worker.id)}
              >
                <TableCell className="font-mono text-xs">{worker.id.slice(-8)}</TableCell>
                <TableCell>
                  <Badge variant="outline" className={cn(workerStatusColor(worker.status))}>
                    {worker.status}
                  </Badge>
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <Progress value={utilization} className="w-16" />
                    <span className="text-xs text-muted-foreground">
                      {worker.usedSlots}/{worker.capacity}
                    </span>
                  </div>
                </TableCell>
                <TableCell className="text-sm">{worker.connectionMode ?? '-'}</TableCell>
                <TableCell className="text-sm">{worker.weight ?? '-'}</TableCell>
                <TableCell className="text-sm text-muted-foreground">
                  {worker.lastHeartbeatAt ? formatRelativeTime(worker.lastHeartbeatAt) : '-'}
                </TableCell>
                <TableCell>
                  <div className="flex gap-1" onClick={(e) => e.stopPropagation()}>
                    <Button
                      variant="outline"
                      size="xs"
                      disabled={drain.isPending || worker.status === 'offline'}
                      onClick={() => handleDrainToggle(worker)}
                    >
                      {worker.status === 'draining' ? 'Resume' : 'Drain'}
                    </Button>
                    <Button
                      variant="destructive"
                      size="xs"
                      disabled={disconnect.isPending}
                      onClick={() => setDisconnectTarget(worker.id)}
                    >
                      Disconnect
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>

      {/* Disconnect confirmation dialog */}
      <Dialog open={!!disconnectTarget} onOpenChange={(open) => { if (!open) setDisconnectTarget(null) }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Disconnect Worker</DialogTitle>
            <DialogDescription>
              Are you sure you want to disconnect worker{' '}
              <span className="font-mono">{disconnectTarget?.slice(-8)}</span>? This will terminate
              the connection immediately.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDisconnectTarget(null)}>
              Cancel
            </Button>
            <Button variant="destructive" onClick={confirmDisconnect} disabled={disconnect.isPending}>
              {disconnect.isPending ? 'Disconnecting...' : 'Disconnect'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
