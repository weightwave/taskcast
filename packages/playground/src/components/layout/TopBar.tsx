import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useConnectionStore, usePanelStore } from '@/stores'
import type { PanelType } from '@/stores'

const roleOptions: { type: PanelType; label: string }[] = [
  { type: 'backend', label: 'Backend (REST)' },
  { type: 'browser', label: 'Browser (SSE)' },
  { type: 'worker-pull', label: 'Worker (Pull)' },
  { type: 'worker-ws', label: 'Worker (WS)' },
]

export function TopBar() {
  const { mode, baseUrl, token, connected, setMode, setBaseUrl, setToken } =
    useConnectionStore()
  const { addPanel } = usePanelStore()

  return (
    <div className="flex items-center gap-3 border-b px-4 py-2">
      <span className="text-sm font-semibold whitespace-nowrap">
        Taskcast Playground
      </span>

      <Select
        value={mode}
        onValueChange={(v) => setMode(v as 'embedded' | 'external')}
      >
        <SelectTrigger className="w-[130px]" size="sm">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="embedded">Embedded</SelectItem>
          <SelectItem value="external">External</SelectItem>
        </SelectContent>
      </Select>

      {mode === 'embedded' ? (
        <span className="text-xs text-muted-foreground">{baseUrl}</span>
      ) : (
        <Input
          placeholder="http://localhost:3000/taskcast"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          className="w-[260px]"
        />
      )}

      <Badge variant={connected ? 'default' : 'secondary'}>
        {connected ? 'Connected' : 'Disconnected'}
      </Badge>

      <div className="flex-1" />

      <Input
        type="password"
        placeholder="JWT Token (optional)"
        value={token}
        onChange={(e) => setToken(e.target.value)}
        className="w-[200px]"
      />

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm">
            + Add Role
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {roleOptions.map((opt) => (
            <DropdownMenuItem
              key={opt.type}
              onSelect={() => addPanel(opt.type)}
            >
              {opt.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}
