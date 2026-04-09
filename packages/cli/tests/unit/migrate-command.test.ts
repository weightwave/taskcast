import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

// Helper: make process.exit throw so execution stops (matching real behavior)
class ExitError extends Error {
  code: number
  constructor(code: number) {
    super(`process.exit(${code})`)
    this.code = code
  }
}

// Mock heavy dependencies
const mockSqlEnd = vi.fn().mockResolvedValue(undefined)
const mockSqlUnsafe = vi.fn()

vi.mock('postgres', () => ({
  default: vi.fn().mockImplementation(() => ({
    unsafe: (...args: unknown[]) => mockSqlUnsafe(...args),
    end: () => mockSqlEnd(),
  })),
}))

vi.mock('url', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    fileURLToPath: () => '/fake/packages/cli/dist/commands/migrate.js',
  }
})

const mockLoadConfigFile = vi.fn()
vi.mock('@taskcast/core', () => ({
  loadConfigFile: (...args: unknown[]) => mockLoadConfigFile(...args),
}))

const mockBuildMigrationFiles = vi.fn()
const mockRunMigrations = vi.fn()
vi.mock('@taskcast/postgres', () => ({
  buildMigrationFiles: (...args: unknown[]) => mockBuildMigrationFiles(...args),
  runMigrations: (...args: unknown[]) => mockRunMigrations(...args),
}))

vi.mock('../../src/generated-migrations.js', () => ({
  EMBEDDED_MIGRATIONS: [
    { filename: '001_initial.sql', sql: 'CREATE TABLE ...' },
  ],
}))

const mockPromptConfirm = vi.fn()
vi.mock('../../src/utils.js', () => ({
  promptConfirm: (...args: unknown[]) => mockPromptConfirm(...args),
}))

// Import after mocks
import { registerMigrateCommand } from '../../src/commands/migrate.js'

describe('registerMigrateCommand', () => {
  let exitSpy: ReturnType<typeof vi.spyOn>
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(((code?: number) => {
      throw new ExitError(code ?? 0)
    }) as never)
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})

    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })
    vi.clearAllMocks()
  })

  afterEach(() => {
    exitSpy.mockRestore()
    logSpy.mockRestore()
    errorSpy.mockRestore()
  })

  it('exits with 1 when no postgres URL found', async () => {
    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    // Remove env var if set
    const origEnv = process.env['TASKCAST_POSTGRES_URL']
    delete process.env['TASKCAST_POSTGRES_URL']

    try {
      await program.parseAsync(['node', 'test', 'migrate'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    } finally {
      if (origEnv !== undefined) process.env['TASKCAST_POSTGRES_URL'] = origEnv
    }

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('No Postgres URL found'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('reports up to date when no pending migrations', async () => {
    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })
    mockSqlUnsafe.mockResolvedValueOnce(undefined) // CREATE TABLE
    mockSqlUnsafe.mockResolvedValueOnce([{ version: 1 }]) // SELECT applied
    mockBuildMigrationFiles.mockReturnValue([{ version: 1, filename: '001_init.sql' }])

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    await program.parseAsync(['node', 'test', 'migrate', '--url', 'postgres://localhost/test'])

    expect(logSpy).toHaveBeenCalledWith('[taskcast] Database is up to date.')
    expect(mockSqlEnd).toHaveBeenCalled()
  })

  it('runs pending migrations with --yes flag', async () => {
    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })
    mockSqlUnsafe.mockResolvedValueOnce(undefined) // CREATE TABLE
    mockSqlUnsafe.mockResolvedValueOnce([]) // no applied
    mockBuildMigrationFiles.mockReturnValue([
      { version: 1, filename: '001_init.sql' },
      { version: 2, filename: '002_add_index.sql' },
    ])
    mockRunMigrations.mockResolvedValue({ applied: ['001_init.sql', '002_add_index.sql'] })

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    await program.parseAsync(['node', 'test', 'migrate', '--url', 'postgres://localhost/test', '--yes'])

    expect(mockRunMigrations).toHaveBeenCalled()
    expect(logSpy).toHaveBeenCalledWith('  Applied 001_init.sql')
    expect(logSpy).toHaveBeenCalledWith('  Applied 002_add_index.sql')
    expect(logSpy).toHaveBeenCalledWith('[taskcast] Applied 2 migration(s) successfully.')
  })

  it('prompts for confirmation and cancels when declined', async () => {
    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })
    mockSqlUnsafe.mockResolvedValueOnce(undefined) // CREATE TABLE
    mockSqlUnsafe.mockResolvedValueOnce([]) // no applied
    mockBuildMigrationFiles.mockReturnValue([
      { version: 1, filename: '001_init.sql' },
    ])
    mockPromptConfirm.mockResolvedValue(false)

    // Set stdin.isTTY to true so it doesn't bail early
    const origTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'migrate', '--url', 'postgres://localhost/test'])
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: origTTY, configurable: true })
    }

    expect(logSpy).toHaveBeenCalledWith('[taskcast] Migration cancelled.')
    expect(mockRunMigrations).not.toHaveBeenCalled()
  })

  it('exits with 1 when no TTY and no --yes flag', async () => {
    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })
    mockSqlUnsafe.mockResolvedValueOnce(undefined) // CREATE TABLE
    mockSqlUnsafe.mockResolvedValueOnce([]) // no applied
    mockBuildMigrationFiles.mockReturnValue([
      { version: 1, filename: '001_init.sql' },
    ])

    const origTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: false, configurable: true })

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'migrate', '--url', 'postgres://localhost/test'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: origTTY, configurable: true })
    }

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('No TTY detected'))
    expect(exitSpy).toHaveBeenCalledWith(1)
  })

  it('exits with 1 on migration error', async () => {
    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })
    mockSqlUnsafe.mockRejectedValue(new Error('connection refused'))
    mockBuildMigrationFiles.mockReturnValue([])

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'migrate', '--url', 'postgres://localhost/test'])
    } catch (e) {
      if (!(e instanceof ExitError && e.code === 1)) throw e
    }

    expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('Migration failed'))
    expect(exitSpy).toHaveBeenCalledWith(1)
    expect(mockSqlEnd).toHaveBeenCalled()
  })

  it('uses config file URL when --url not provided', async () => {
    const postgres = (await import('postgres')).default as unknown as ReturnType<typeof vi.fn>
    mockLoadConfigFile.mockResolvedValue({
      config: { adapters: { longTermStore: { url: 'postgres://config-host/db' } } },
      source: 'file',
    })
    mockSqlUnsafe.mockResolvedValueOnce(undefined)
    mockSqlUnsafe.mockResolvedValueOnce([{ version: 1 }])
    mockBuildMigrationFiles.mockReturnValue([{ version: 1, filename: '001_init.sql' }])

    const origEnv = process.env['TASKCAST_POSTGRES_URL']
    delete process.env['TASKCAST_POSTGRES_URL']

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'migrate'])
    } finally {
      if (origEnv !== undefined) process.env['TASKCAST_POSTGRES_URL'] = origEnv
    }

    expect(postgres).toHaveBeenCalledWith('postgres://config-host/db')
  })

  it('displays pending migration filenames before running', async () => {
    mockLoadConfigFile.mockResolvedValue({ config: {}, source: 'none' })
    mockSqlUnsafe.mockResolvedValueOnce(undefined)
    mockSqlUnsafe.mockResolvedValueOnce([])
    mockBuildMigrationFiles.mockReturnValue([
      { version: 1, filename: '001_init.sql' },
    ])
    mockPromptConfirm.mockResolvedValue(true)
    mockRunMigrations.mockResolvedValue({ applied: ['001_init.sql'] })

    const origTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    const program = new Command()
    program.exitOverride()
    registerMigrateCommand(program)

    try {
      await program.parseAsync(['node', 'test', 'migrate', '--url', 'postgres://localhost/test'])
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: origTTY, configurable: true })
    }

    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Pending migrations'))
    expect(logSpy).toHaveBeenCalledWith('  001_init.sql')
  })
})
