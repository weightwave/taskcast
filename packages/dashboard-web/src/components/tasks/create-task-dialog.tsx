import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { useCreateTask } from '@/hooks/use-tasks'

export function CreateTaskDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [type, setType] = useState('')
  const [paramsText, setParamsText] = useState('')
  const [ttl, setTtl] = useState('')
  const [tags, setTags] = useState('')
  const [paramsError, setParamsError] = useState<string | null>(null)

  const createTask = useCreateTask()

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    setParamsError(null)

    let params: Record<string, unknown> | undefined
    if (paramsText.trim()) {
      try {
        params = JSON.parse(paramsText)
      } catch {
        setParamsError('Invalid JSON')
        return
      }
    }

    const input: { type?: string; params?: Record<string, unknown>; ttl?: number; tags?: string[] } = {}
    if (type.trim()) input.type = type.trim()
    if (params) input.params = params
    if (ttl.trim()) input.ttl = Number(ttl)
    if (tags.trim()) input.tags = tags.split(',').map((t) => t.trim()).filter(Boolean)

    createTask.mutate(input, {
      onSuccess: () => {
        onOpenChange(false)
        setType('')
        setParamsText('')
        setTtl('')
        setTags('')
      },
    })
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create Task</DialogTitle>
          <DialogDescription>Create a new task to be tracked by Taskcast.</DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <label htmlFor="task-type" className="text-sm font-medium">
              Type
            </label>
            <Input
              id="task-type"
              placeholder="e.g. llm.chat"
              value={type}
              onChange={(e) => setType(e.target.value)}
            />
          </div>

          <div className="space-y-2">
            <label htmlFor="task-params" className="text-sm font-medium">
              Params (JSON)
            </label>
            <textarea
              id="task-params"
              className="flex min-h-[80px] w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 outline-none disabled:cursor-not-allowed disabled:opacity-50"
              placeholder='{"model": "gpt-4"}'
              value={paramsText}
              onChange={(e) => {
                setParamsText(e.target.value)
                setParamsError(null)
              }}
            />
            {paramsError && <p className="text-xs text-destructive">{paramsError}</p>}
          </div>

          <div className="space-y-2">
            <label htmlFor="task-ttl" className="text-sm font-medium">
              TTL (seconds)
            </label>
            <Input
              id="task-ttl"
              type="number"
              placeholder="300"
              value={ttl}
              onChange={(e) => setTtl(e.target.value)}
            />
          </div>

          <div className="space-y-2">
            <label htmlFor="task-tags" className="text-sm font-medium">
              Tags (comma-separated)
            </label>
            <Input
              id="task-tags"
              placeholder="production, urgent"
              value={tags}
              onChange={(e) => setTags(e.target.value)}
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={createTask.isPending}>
              {createTask.isPending ? 'Creating...' : 'Create'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
