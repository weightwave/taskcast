import { Button } from '@/components/ui/button'
import { useTransitionTask } from '@/hooks/use-tasks'

/**
 * Valid transitions from the Taskcast state machine.
 */
const TRANSITIONS: Record<string, string[]> = {
  pending: ['running', 'cancelled'],
  assigned: ['running', 'cancelled'],
  running: ['completed', 'failed', 'cancelled', 'paused'],
  paused: ['running', 'cancelled'],
  blocked: ['running', 'cancelled'],
}

const TRANSITION_STYLE: Record<string, { variant: 'default' | 'secondary' | 'destructive' | 'outline' }> = {
  running: { variant: 'default' },
  completed: { variant: 'secondary' },
  failed: { variant: 'destructive' },
  cancelled: { variant: 'destructive' },
  paused: { variant: 'outline' },
}

export function TaskActions({ taskId, currentStatus }: { taskId: string; currentStatus: string }) {
  const transition = useTransitionTask()
  const validTargets = TRANSITIONS[currentStatus]

  if (!validTargets || validTargets.length === 0) {
    return <p className="text-sm text-muted-foreground">No available transitions.</p>
  }

  return (
    <div className="flex flex-wrap gap-2">
      {validTargets.map((target) => {
        const style = TRANSITION_STYLE[target] ?? { variant: 'outline' as const }
        return (
          <Button
            key={target}
            variant={style.variant}
            size="sm"
            disabled={transition.isPending}
            onClick={() => transition.mutate({ taskId, status: target })}
          >
            {target.charAt(0).toUpperCase() + target.slice(1)}
          </Button>
        )
      })}
    </div>
  )
}
