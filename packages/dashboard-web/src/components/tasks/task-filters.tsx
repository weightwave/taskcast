import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

const STATUSES = [
  { value: '_all', label: 'All statuses' },
  { value: 'pending', label: 'Pending' },
  { value: 'assigned', label: 'Assigned' },
  { value: 'running', label: 'Running' },
  { value: 'paused', label: 'Paused' },
  { value: 'blocked', label: 'Blocked' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
  { value: 'timeout', label: 'Timeout' },
  { value: 'cancelled', label: 'Cancelled' },
]

export function TaskFilters({
  status,
  type,
  onStatusChange,
  onTypeChange,
  onCreateClick,
}: {
  status: string
  type: string
  onStatusChange: (value: string) => void
  onTypeChange: (value: string) => void
  onCreateClick: () => void
}) {
  return (
    <div className="flex items-center gap-3">
      <Select value={status} onValueChange={onStatusChange}>
        <SelectTrigger className="w-[160px]">
          <SelectValue placeholder="All statuses" />
        </SelectTrigger>
        <SelectContent>
          {STATUSES.map((s) => (
            <SelectItem key={s.value} value={s.value}>
              {s.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <Input
        placeholder="Filter by type..."
        value={type}
        onChange={(e) => onTypeChange(e.target.value)}
        className="max-w-[200px]"
      />

      <Button onClick={onCreateClick}>Create Task</Button>
    </div>
  )
}
