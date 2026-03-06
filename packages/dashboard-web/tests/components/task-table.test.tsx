import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { TaskTable } from '@/components/tasks/task-table'
import type { DashboardTask } from '@/types'

const mockTasks: DashboardTask[] = [
  {
    id: 'task-001',
    type: 'llm.chat',
    status: 'running',
    hot: true,
    subscriberCount: 3,
    workerId: 'worker-abc',
    createdAt: Date.now() - 60_000,
    updatedAt: Date.now(),
  } as DashboardTask,
  {
    id: 'task-002',
    type: 'batch.process',
    status: 'completed',
    hot: false,
    subscriberCount: 0,
    workerId: undefined,
    createdAt: Date.now() - 120_000,
    updatedAt: Date.now(),
  } as DashboardTask,
]

describe('TaskTable', () => {
  it('renders empty state', () => {
    render(<TaskTable tasks={[]} selectedTaskId={null} onSelect={vi.fn()} />)

    expect(screen.getByText('No tasks found.')).toBeDefined()
  })

  it('renders task rows with status badges and type text', () => {
    render(
      <TaskTable tasks={mockTasks} selectedTaskId={null} onSelect={vi.fn()} />,
    )

    // Check status badges
    expect(screen.getByText('running')).toBeDefined()
    expect(screen.getByText('completed')).toBeDefined()

    // Check type text
    expect(screen.getByText('llm.chat')).toBeDefined()
    expect(screen.getByText('batch.process')).toBeDefined()
  })

  it('shows Hot/Cold badges', () => {
    render(
      <TaskTable tasks={mockTasks} selectedTaskId={null} onSelect={vi.fn()} />,
    )

    expect(screen.getByText('Hot')).toBeDefined()
    expect(screen.getByText('Cold')).toBeDefined()
  })

  it('calls onSelect when row is clicked', () => {
    const onSelect = vi.fn()
    render(
      <TaskTable tasks={mockTasks} selectedTaskId={null} onSelect={onSelect} />,
    )

    // Click the row containing task-001's truncated ID
    const row = screen.getByText('task-001'.slice(-8)).closest('tr')
    expect(row).not.toBe(null)
    fireEvent.click(row!)

    expect(onSelect).toHaveBeenCalledWith('task-001')
  })
})
