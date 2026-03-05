import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { Shell } from './components/layout/shell'
import { OverviewPage } from './pages/overview'
import { TasksPage } from './pages/tasks'
import { EventsPage } from './pages/events'
import { WorkersPage } from './pages/workers'
import { LoginPage } from './pages/login'
import { useConnectionStore } from './stores/connection'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchInterval: 5000,
      retry: 1,
    },
  },
})

export function App() {
  const connected = useConnectionStore((s) => s.connected)

  if (!connected) {
    return <LoginPage />
  }

  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Shell>
          <Routes>
            <Route path="/" element={<OverviewPage />} />
            <Route path="/tasks" element={<TasksPage />} />
            <Route path="/tasks/:taskId" element={<TasksPage />} />
            <Route path="/events" element={<EventsPage />} />
            <Route path="/workers" element={<WorkersPage />} />
          </Routes>
        </Shell>
      </BrowserRouter>
    </QueryClientProvider>
  )
}
