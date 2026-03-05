import { useState } from 'react'
import { useParams, useNavigate } from 'react-router-dom'
import { useTasksQuery } from '@/hooks/use-tasks'
import { ScrollArea } from '@/components/ui/scroll-area'
import { TaskFilters } from '@/components/tasks/task-filters'
import { TaskTable } from '@/components/tasks/task-table'
import { TaskDetail } from '@/components/tasks/task-detail'
import { CreateTaskDialog } from '@/components/tasks/create-task-dialog'

export function TasksPage() {
  const { taskId } = useParams<{ taskId?: string }>()
  const navigate = useNavigate()

  const [statusFilter, setStatusFilter] = useState('_all')
  const [typeFilter, setTypeFilter] = useState('')
  const [createOpen, setCreateOpen] = useState(false)

  const filter: { status?: string; type?: string } = {}
  if (statusFilter && statusFilter !== '_all') filter.status = statusFilter
  if (typeFilter.trim()) filter.type = typeFilter.trim()

  const { data: tasks = [] } = useTasksQuery(Object.keys(filter).length > 0 ? filter : undefined)

  const selectedTaskId = taskId ?? null

  function handleSelect(id: string) {
    navigate(`/tasks/${id}`)
  }

  return (
    <div className="flex h-full flex-col gap-4">
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold tracking-tight">Tasks</h2>
        <TaskFilters
          status={statusFilter}
          type={typeFilter}
          onStatusChange={setStatusFilter}
          onTypeChange={setTypeFilter}
          onCreateClick={() => setCreateOpen(true)}
        />
      </div>

      <div className="flex min-h-0 flex-1 gap-4">
        {/* Left: task table */}
        <div className={selectedTaskId ? 'w-1/2' : 'w-full'}>
          <ScrollArea className="h-full">
            <TaskTable
              tasks={tasks}
              selectedTaskId={selectedTaskId}
              onSelect={handleSelect}
            />
          </ScrollArea>
        </div>

        {/* Right: task detail */}
        {selectedTaskId && (
          <div className="w-1/2 border-l pl-4">
            <ScrollArea className="h-full">
              <TaskDetail taskId={selectedTaskId} />
            </ScrollArea>
          </div>
        )}
      </div>

      <CreateTaskDialog open={createOpen} onOpenChange={setCreateOpen} />
    </div>
  )
}
