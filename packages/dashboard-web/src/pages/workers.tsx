import { useState } from 'react'
import { useWorkersQuery } from '@/hooks/use-workers'
import { ScrollArea } from '@/components/ui/scroll-area'
import { WorkerTable } from '@/components/workers/worker-table'
import { WorkerDetail } from '@/components/workers/worker-detail'

export function WorkersPage() {
  const { data: workers = [] } = useWorkersQuery()
  const [selectedWorkerId, setSelectedWorkerId] = useState<string | null>(null)

  const selectedWorker = selectedWorkerId
    ? workers.find((w) => w.id === selectedWorkerId) ?? null
    : null

  return (
    <div className="flex h-full flex-col gap-4">
      <h2 className="text-2xl font-bold tracking-tight">Workers</h2>

      <div className="flex min-h-0 flex-1 gap-4">
        {/* Left: worker table */}
        <div className={selectedWorker ? 'w-3/5' : 'w-full'}>
          <ScrollArea className="h-full">
            <WorkerTable
              workers={workers}
              selectedWorkerId={selectedWorkerId}
              onSelect={setSelectedWorkerId}
            />
          </ScrollArea>
        </div>

        {/* Right: worker detail */}
        {selectedWorker && (
          <div className="w-2/5 border-l pl-4">
            <ScrollArea className="h-full">
              <WorkerDetail worker={selectedWorker} />
            </ScrollArea>
          </div>
        )}
      </div>
    </div>
  )
}
