import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useDataStore } from '@/stores'

/* ------------------------------------------------------------------ */
/*  tryParseJson                                                       */
/* ------------------------------------------------------------------ */

export function tryParseJson(text: string): { ok: true; value: unknown } | { ok: false; error: string } {
  const trimmed = text.trim()
  if (trimmed === '') return { ok: true, value: undefined }
  try {
    return { ok: true, value: JSON.parse(trimmed) }
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
}

/* ------------------------------------------------------------------ */
/*  ApiResponse type + ResponseDisplay                                 */
/* ------------------------------------------------------------------ */

export interface ApiResponse {
  status: number
  body: unknown
}

function statusBadgeVariant(code: number): 'default' | 'secondary' | 'destructive' | 'outline' {
  if (code >= 200 && code < 300) return 'default'
  if (code >= 400 && code < 500) return 'secondary'
  if (code >= 500) return 'destructive'
  return 'outline'
}

export function ResponseDisplay({ response }: { response: ApiResponse | null }) {
  if (!response) return null
  return (
    <div className="mt-2 space-y-1">
      <Badge variant={statusBadgeVariant(response.status)}>{response.status}</Badge>
      <ScrollArea className="max-h-48 rounded border">
        <pre className="p-2 text-xs whitespace-pre-wrap break-all">
          {JSON.stringify(response.body, null, 2)}
        </pre>
      </ScrollArea>
    </div>
  )
}

/* ------------------------------------------------------------------ */
/*  TaskIdField                                                        */
/* ------------------------------------------------------------------ */

export function TaskIdField({
  value,
  onChange,
}: {
  value: string
  onChange: (v: string) => void
}) {
  const { tasks } = useDataStore()

  return (
    <div className="space-y-1.5">
      <Label>Task ID</Label>
      {tasks.length > 0 && (
        <Select value={value} onValueChange={onChange}>
          <SelectTrigger className="w-full">
            <SelectValue placeholder="Select a task..." />
          </SelectTrigger>
          <SelectContent>
            {tasks.map((t) => (
              <SelectItem key={t.id} value={t.id}>
                {t.id} ({t.type} / {t.status})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      <Input
        placeholder="Or type a task ID..."
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  )
}
