import { describe, it, expect } from 'vitest'
import { formatTaskList, formatTaskInspect } from '../../src/commands/tasks.js'

describe('formatTaskList', () => {
  it('formats multiple tasks into a table', () => {
    const tasks = [
      { id: '01JXXXXXXXXXXXXXXXXXX', type: 'llm.chat', status: 'running', createdAt: 1741355401000 },
      { id: '01JYYYYYYYYYYYYYYYYYY', type: 'agent.step', status: 'completed', createdAt: 1741355335000 },
    ]
    const output = formatTaskList(tasks)

    expect(output).toContain('ID')
    expect(output).toContain('TYPE')
    expect(output).toContain('STATUS')
    expect(output).toContain('CREATED')
    expect(output).toContain('01JXXXXXXXXXXXXXXXXXX')
    expect(output).toContain('llm.chat')
    expect(output).toContain('running')
    expect(output).toContain('01JYYYYYYYYYYYYYYYYYY')
    expect(output).toContain('agent.step')
    expect(output).toContain('completed')
  })

  it('returns "No tasks found." for empty array', () => {
    const output = formatTaskList([])
    expect(output).toBe('No tasks found.')
  })

  it('handles tasks with missing type', () => {
    const tasks = [
      { id: '01JABCDEF', status: 'pending', createdAt: 1741355401000 },
    ]
    const output = formatTaskList(tasks as any)
    expect(output).toContain('01JABCDEF')
    expect(output).toContain('pending')
  })

  it('includes header row as first line', () => {
    const tasks = [
      { id: '01JXXXXXXXXXXXXXXXXXX', type: 'llm.chat', status: 'running', createdAt: 1741355401000 },
    ]
    const output = formatTaskList(tasks)
    const lines = output.split('\n')
    expect(lines[0]).toContain('ID')
    expect(lines[0]).toContain('TYPE')
    expect(lines[0]).toContain('STATUS')
    expect(lines[0]).toContain('CREATED')
    expect(lines.length).toBe(2) // header + 1 row
  })
})

describe('formatTaskInspect', () => {
  it('formats task with events', () => {
    const task = {
      id: '01JXXXXXXXXXXXXXXXXXX',
      type: 'llm.chat',
      status: 'running',
      params: { prompt: 'Hello' },
      createdAt: 1741355401000,
    }
    const events = [
      { type: 'llm.delta', level: 'info', seriesId: 'response', timestamp: 1741355402000 },
      { type: 'llm.delta', level: 'info', seriesId: 'response', timestamp: 1741355402500 },
    ]
    const output = formatTaskInspect(task, events)

    expect(output).toContain('Task: 01JXXXXXXXXXXXXXXXXXX')
    expect(output).toContain('Type:    llm.chat')
    expect(output).toContain('Status:  running')
    expect(output).toContain('Params:  {"prompt":"Hello"}')
    expect(output).toContain('Recent Events (last 2):')
    expect(output).toContain('#0')
    expect(output).toContain('#1')
    expect(output).toContain('llm.delta')
    expect(output).toContain('series:response')
  })

  it('formats task with no events', () => {
    const task = {
      id: '01JXXXXXXXXXXXXXXXXXX',
      type: 'llm.chat',
      status: 'pending',
      params: null,
      createdAt: 1741355401000,
    }
    const output = formatTaskInspect(task, [])

    expect(output).toContain('Task: 01JXXXXXXXXXXXXXXXXXX')
    expect(output).toContain('Type:    llm.chat')
    expect(output).toContain('Status:  pending')
    expect(output).toContain('No events.')
    expect(output).not.toContain('Recent Events')
  })

  it('shows only last 5 events when more are present', () => {
    const task = {
      id: '01JABCDEF',
      type: 'batch',
      status: 'running',
      params: {},
      createdAt: 1741355401000,
    }
    const events = Array.from({ length: 8 }, (_, i) => ({
      type: `step.${i}`,
      level: 'info',
      timestamp: 1741355402000 + i * 1000,
    }))
    const output = formatTaskInspect(task, events)

    expect(output).toContain('Recent Events (last 5):')
    // Should contain events 3-7 (the last 5), indexed as #0-#4
    expect(output).toContain('#0')
    expect(output).toContain('#4')
    expect(output).not.toContain('#5')
    // The last 5 events are step.3 through step.7
    expect(output).toContain('step.3')
    expect(output).toContain('step.7')
    expect(output).not.toContain('step.2')
  })

  it('handles task with undefined params', () => {
    const task = {
      id: '01JABCDEF',
      type: 'simple',
      status: 'completed',
      createdAt: 1741355401000,
    }
    const output = formatTaskInspect(task, [])

    expect(output).toContain('Params:  ')
    expect(output).toContain('Status:  completed')
  })

  it('handles events without seriesId', () => {
    const task = {
      id: '01JABCDEF',
      type: 'llm.chat',
      status: 'running',
      params: {},
      createdAt: 1741355401000,
    }
    const events = [
      { type: 'log', level: 'info', timestamp: 1741355402000 },
    ]
    const output = formatTaskInspect(task, events)

    expect(output).toContain('#0')
    expect(output).toContain('log')
    expect(output).not.toContain('series:')
  })
})
