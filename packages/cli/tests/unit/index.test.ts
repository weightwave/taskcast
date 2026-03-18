import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// We need to mock all the heavy imports that index.ts pulls in
// The simplest approach: mock the register functions and commander

// Mock all register functions to avoid importing heavy deps
vi.mock('../../src/commands/start.js', () => ({
  registerStartCommand: vi.fn(),
}))
vi.mock('../../src/commands/migrate.js', () => ({
  registerMigrateCommand: vi.fn(),
}))
vi.mock('../../src/commands/playground.js', () => ({
  registerPlaygroundCommand: vi.fn(),
}))
vi.mock('../../src/commands/ui.js', () => ({
  registerUiCommand: vi.fn(),
}))
vi.mock('../../src/commands/node.js', () => ({
  registerNodeCommand: vi.fn(),
}))
vi.mock('../../src/commands/ping.js', () => ({
  registerPingCommand: vi.fn(),
}))
vi.mock('../../src/commands/doctor.js', () => ({
  registerDoctorCommand: vi.fn(),
}))
vi.mock('../../src/commands/logs.js', () => ({
  registerLogsCommand: vi.fn(),
  registerTailCommand: vi.fn(),
}))
vi.mock('../../src/commands/tasks.js', () => ({
  registerTasksCommand: vi.fn(),
}))

// Mock commander to capture the program
const mockParse = vi.fn()
const mockCommand = vi.fn().mockReturnValue({
  description: vi.fn().mockReturnValue({
    action: vi.fn(),
  }),
})
const mockProgram = {
  name: vi.fn().mockReturnThis(),
  description: vi.fn().mockReturnThis(),
  version: vi.fn().mockReturnThis(),
  command: mockCommand,
  parse: mockParse,
}

vi.mock('commander', () => ({
  Command: vi.fn().mockImplementation(() => mockProgram),
}))

describe('index.ts', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation((() => {}) as never)
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.clearAllMocks()
  })

  afterEach(() => {
    exitSpy.mockRestore()
    errorSpy.mockRestore()
  })

  it('registers all commands and calls parse', async () => {
    await import('../../src/index.js')

    const { registerStartCommand } = await import('../../src/commands/start.js')
    const { registerMigrateCommand } = await import('../../src/commands/migrate.js')
    const { registerPlaygroundCommand } = await import('../../src/commands/playground.js')
    const { registerUiCommand } = await import('../../src/commands/ui.js')
    const { registerNodeCommand } = await import('../../src/commands/node.js')
    const { registerPingCommand } = await import('../../src/commands/ping.js')
    const { registerDoctorCommand } = await import('../../src/commands/doctor.js')
    const { registerLogsCommand, registerTailCommand } = await import('../../src/commands/logs.js')
    const { registerTasksCommand } = await import('../../src/commands/tasks.js')

    expect(registerStartCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerMigrateCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerPlaygroundCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerUiCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerNodeCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerPingCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerDoctorCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerLogsCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerTailCommand).toHaveBeenCalledWith(mockProgram)
    expect(registerTasksCommand).toHaveBeenCalledWith(mockProgram)

    // Placeholder commands: daemon, stop, status
    expect(mockCommand).toHaveBeenCalledWith('daemon')
    expect(mockCommand).toHaveBeenCalledWith('stop')
    expect(mockCommand).toHaveBeenCalledWith('status')

    expect(mockParse).toHaveBeenCalled()
  })
})
