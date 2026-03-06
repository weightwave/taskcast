/**
 * Map task status to shadcn Badge variant.
 */
export function statusBadgeVariant(status: string): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (status) {
    case 'running':
    case 'assigned':
      return 'default'
    case 'completed':
      return 'secondary'
    case 'failed':
    case 'timeout':
    case 'cancelled':
      return 'destructive'
    case 'pending':
    case 'paused':
    case 'blocked':
    default:
      return 'outline'
  }
}

/**
 * Map task status to a tailwind text color class.
 */
export function statusColor(status: string): string {
  switch (status) {
    case 'pending':
      return 'text-yellow-600 dark:text-yellow-400'
    case 'assigned':
      return 'text-cyan-600 dark:text-cyan-400'
    case 'running':
      return 'text-blue-600 dark:text-blue-400'
    case 'completed':
      return 'text-green-600 dark:text-green-400'
    case 'failed':
      return 'text-red-600 dark:text-red-400'
    case 'timeout':
      return 'text-orange-600 dark:text-orange-400'
    case 'cancelled':
      return 'text-gray-600 dark:text-gray-400'
    case 'paused':
      return 'text-purple-600 dark:text-purple-400'
    case 'blocked':
      return 'text-amber-600 dark:text-amber-400'
    default:
      return 'text-muted-foreground'
  }
}

/**
 * Map worker status to a tailwind color class.
 */
export function workerStatusColor(status: string): string {
  switch (status) {
    case 'idle':
      return 'text-green-600 dark:text-green-400'
    case 'busy':
      return 'text-blue-600 dark:text-blue-400'
    case 'draining':
      return 'text-yellow-600 dark:text-yellow-400'
    case 'offline':
      return 'text-red-600 dark:text-red-400'
    default:
      return 'text-muted-foreground'
  }
}

/**
 * Map event level to badge styling.
 */
export function levelBadgeClass(level: string): string {
  switch (level) {
    case 'debug':
      return 'bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300'
    case 'info':
      return 'bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300'
    case 'warn':
      return 'bg-yellow-100 text-yellow-700 dark:bg-yellow-900 dark:text-yellow-300'
    case 'error':
      return 'bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300'
    default:
      return 'bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300'
  }
}
