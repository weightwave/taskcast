import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { usePanelStore } from '@/stores'
import type { Panel } from '@/stores'

export function PanelAuthConfig({ panel }: { panel: Panel }) {
  const updatePanel = usePanelStore((s) => s.updatePanel)

  return (
    <div className="flex items-center gap-1">
      <Select value={panel.useAuth} onValueChange={(v) => updatePanel(panel.id, { useAuth: v as Panel['useAuth'] })}>
        <SelectTrigger className="h-6 w-[90px] text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="global">Global</SelectItem>
          <SelectItem value="custom">Custom</SelectItem>
          <SelectItem value="none">No Auth</SelectItem>
        </SelectContent>
      </Select>
      {panel.useAuth === 'custom' && (
        <Input
          className="h-6 w-[140px] text-xs"
          placeholder="Bearer token"
          type="password"
          value={panel.customToken ?? ''}
          onChange={(e) => updatePanel(panel.id, { customToken: e.target.value })}
        />
      )}
    </div>
  )
}
