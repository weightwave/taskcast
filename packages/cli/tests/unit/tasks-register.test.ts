import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

class ExitError extends Error {
  code: number
  constructor(code: number) {
    super(`process.exit(${code})`)
    this.code = code
  }
}

// Mock NodeConfigManager
const mockGet = vi.fn()
const mockGetCurrent = vi.fn()

vi.mock('../../src/node-config.js', () => ({
  NodeConfigManager: vi.fn().mockImplementation(() => ({
    get: mockGet,
    getCurrent: mockGetCurrent,
  })),
}))

// Mock createClientFromNodeAsync
const mockCreateClient = vi.fn()

vi.mock('../../src/client.js', () => ({
  createClientFromNodeAsync: (...args: unknown[]) => mockCreateClient(...args),
}))

import { registerTasksCommand } from '../../src/commands/tasks.js'

describe('registerTasksCommand — tasks list', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.clearAllMocks()
  })

  afterEach(() => {
    exitSpy.mockRestore()
    logSpy.mockRestore()
    errorSpy.mockRestore()
  })

  it('lists tasks from current node', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })
    mockCreateClient.mockResolvedValue({
      _request: vi.fn().mockResolvedValue({
        tasks: [
          { id: '01JXX123', type: 'llm.chat', status: 'running', createdAt: 1741355401000 },
        ],
      }),
    })

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    await program.parseAsync(['node', 'test', 'tasks', 'list'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('01JXX123'))
    expect(exitSpy).not.toHaveBeenCalled()
  })

  it('passes status and type filter query params', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })

    let capturedPath = ''
    mockCreateClient.mockResolvedValue({
      _request: vi.fn().mockImplementation((_method: string, path: string) => {
        capturedPath = path
        return Promise.resolve({ tasks: [] })
      }),
    })

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    await program.parseAsync(['node', 'test', 'tasks', 'list', '--status', 'running', '--type', 'llm.*'])

    expect(capturedPath).toContain('status=running')
    expect(capturedPath).toContain('type=llm.*')
  })

  it('limits task list output', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })
    mockCreateClient.mockResolvedValue({
      _request: vi.fn().mockResolvedValue({
        tasks: Array.from({ length: 50 }, (_, i) => ({
          id: `task-${i}`,
          type: 'test',
          status: 'running',
          createdAt: 1741355401000,
        })),
      }),
    })

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    await program.parseAsync(['node', 'test', 'tasks', 'list', '--limit', '5'])

    const output = logSpy.mock.calls[0][0] as string
    // Header + 5 rows
    const lines = output.split('\n')
    expect(lines).toHaveLength(6)
  })

  it('uses --node option', async () => {
    mockGet.mockReturnValue({ url: 'https://prod.example.com' })
    mockCreateClient.mockResolvedValue({
      _request: vi.fn().mockResolvedValue({ tasks: [] }),
    })

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    await program.parseAsync(['node', 'test', 'tasks', 'list', '--node', 'prod'])

    expect(mockGet).toHaveBeenCalledWith('prod')
  })

  it('exits with 1 when --node not found', async () => {
    mockGet.mockReturnValue(undefined)

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'tasks', 'list', '--node', 'ghost'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Node "ghost" not found')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exits with 1 on client error', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })
    mockCreateClient.mockRejectedValue(new Error('ECONNREFUSED'))

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'tasks', 'list'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Error: ECONNREFUSED')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })
})

describe('registerTasksCommand — tasks inspect', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.clearAllMocks()
  })

  afterEach(() => {
    exitSpy.mockRestore()
    logSpy.mockRestore()
    errorSpy.mockRestore()
  })

  it('inspects a task from current node', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })
    mockCreateClient.mockResolvedValue({
      getTask: vi.fn().mockResolvedValue({
        id: '01JXX123',
        type: 'llm.chat',
        status: 'running',
        params: { prompt: 'hi' },
        createdAt: 1741355401000,
      }),
      getHistory: vi.fn().mockResolvedValue([
        { type: 'llm.delta', level: 'info', timestamp: 1741355402000 },
      ]),
    })

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    await program.parseAsync(['node', 'test', 'tasks', 'inspect', '01JXX123'])

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Task: 01JXX123'))
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('llm.chat'))
    expect(exitSpy).not.toHaveBeenCalled()
  })

  it('uses --node option for inspect', async () => {
    mockGet.mockReturnValue({ url: 'https://prod.example.com' })
    mockCreateClient.mockResolvedValue({
      getTask: vi.fn().mockResolvedValue({
        id: '01JXX123',
        type: 'test',
        status: 'pending',
        createdAt: 1741355401000,
      }),
      getHistory: vi.fn().mockResolvedValue([]),
    })

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    await program.parseAsync(['node', 'test', 'tasks', 'inspect', '01JXX123', '--node', 'prod'])

    expect(mockGet).toHaveBeenCalledWith('prod')
  })

  it('exits with 1 when --node not found for inspect', async () => {
    mockGet.mockReturnValue(undefined)

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'tasks', 'inspect', '01JXX123', '--node', 'ghost'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Node "ghost" not found')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exits with 1 on inspect client error', async () => {
    mockGetCurrent.mockReturnValue({ url: 'http://localhost:3721' })
    mockCreateClient.mockRejectedValue(new Error('Task not found'))

    const program = new Command()
    program.exitOverride()
    registerTasksCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'tasks', 'inspect', '01JXX123'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith('Error: Task not found')
    expect(exitSpy).toHaveBeenCalledWith(1)
  })
})
