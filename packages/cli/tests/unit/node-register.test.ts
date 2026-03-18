import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'
import { registerNodeCommand } from '../../src/commands/node.js'

// Mock NodeConfigManager
const mockAdd = vi.fn()
const mockRemove = vi.fn()
const mockUse = vi.fn()
const mockList = vi.fn()

vi.mock('../../src/node-config.js', () => ({
  NodeConfigManager: vi.fn().mockImplementation(() => ({
    add: mockAdd,
    remove: mockRemove,
    use: mockUse,
    list: mockList,
  })),
}))

describe('registerNodeCommand', () => {
  let logSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.clearAllMocks()
  })

  afterEach(() => {
    logSpy.mockRestore()
  })

  it('node add creates an entry with url and token', async () => {
    const program = new Command()
    program.exitOverride()
    registerNodeCommand(program)

    await program.parseAsync(['node', 'test', 'node', 'add', 'prod', '--url', 'https://prod.example.com', '--token', 'my-jwt', '--token-type', 'jwt'])

    expect(mockAdd).toHaveBeenCalledWith('prod', {
      url: 'https://prod.example.com',
      token: 'my-jwt',
      tokenType: 'jwt',
    })
    expect(logSpy).toHaveBeenCalledWith('Added node "prod" \u2192 https://prod.example.com')
  })

  it('node add creates an entry without token', async () => {
    const program = new Command()
    program.exitOverride()
    registerNodeCommand(program)

    await program.parseAsync(['node', 'test', 'node', 'add', 'local', '--url', 'http://localhost:3721'])

    expect(mockAdd).toHaveBeenCalledWith('local', { url: 'http://localhost:3721' })
    expect(logSpy).toHaveBeenCalledWith('Added node "local" \u2192 http://localhost:3721')
  })

  it('node remove removes an entry', async () => {
    const program = new Command()
    program.exitOverride()
    registerNodeCommand(program)

    await program.parseAsync(['node', 'test', 'node', 'remove', 'prod'])

    expect(mockRemove).toHaveBeenCalledWith('prod')
    expect(logSpy).toHaveBeenCalledWith('Removed node "prod"')
  })

  it('node use switches current node', async () => {
    const program = new Command()
    program.exitOverride()
    registerNodeCommand(program)

    await program.parseAsync(['node', 'test', 'node', 'use', 'staging'])

    expect(mockUse).toHaveBeenCalledWith('staging')
    expect(logSpy).toHaveBeenCalledWith('Switched to node "staging"')
  })

  it('node list displays all nodes', async () => {
    mockList.mockReturnValue([
      { name: 'local', url: 'http://localhost:3721', current: true },
      { name: 'prod', url: 'https://prod.example.com', token: 'ey...', tokenType: 'jwt', current: false },
    ])

    const program = new Command()
    program.exitOverride()
    registerNodeCommand(program)

    await program.parseAsync(['node', 'test', 'node', 'list'])

    expect(mockList).toHaveBeenCalled()
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('local'))
  })
})
