import { NavLink } from 'react-router-dom'
import { cn } from '@/lib/utils'

const navItems = [
  { to: '/', label: 'Overview' },
  { to: '/tasks', label: 'Tasks' },
  { to: '/events', label: 'Events' },
  { to: '/workers', label: 'Workers' },
]

export function Sidebar() {
  return (
    <aside className="w-56 border-r bg-muted/40 p-4">
      <h1 className="mb-6 text-lg font-semibold tracking-tight">Taskcast</h1>
      <nav className="space-y-1">
        {navItems.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.to === '/'}
            className={({ isActive }) =>
              cn(
                'block rounded-md px-3 py-2 text-sm font-medium transition-colors',
                isActive
                  ? 'bg-primary text-primary-foreground'
                  : 'text-muted-foreground hover:bg-muted hover:text-foreground',
              )
            }
          >
            {item.label}
          </NavLink>
        ))}
      </nav>
    </aside>
  )
}
