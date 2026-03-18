import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// Mock os.homedir
const mockHomedir = vi.fn().mockReturnValue('/tmp/fake-home')
vi.mock('os', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    homedir: () => mockHomedir(),
  }
})

// Mock readline to test interactive prompts
const mockRlQuestion = vi.fn()
const mockRlClose = vi.fn()
const mockRlOn = vi.fn().mockReturnThis()
vi.mock('readline', async (importOriginal) => {
  const actual = await importOriginal() as Record<string, unknown>
  return {
    ...actual,
    createInterface: vi.fn().mockImplementation(() => ({
      question: mockRlQuestion,
      close: mockRlClose,
      on: mockRlOn,
    })),
  }
})

import { mkdtempSync, rmSync, existsSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import {
  DEFAULT_CONFIG_YAML,
  promptCreateGlobalConfig,
  promptConfirm,
  createDefaultGlobalConfig,
} from '../../src/utils.js'

describe('DEFAULT_CONFIG_YAML', () => {
  it('contains default port and commented sections', () => {
    expect(DEFAULT_CONFIG_YAML).toContain('port: 3721')
    expect(DEFAULT_CONFIG_YAML).toContain('# auth:')
    expect(DEFAULT_CONFIG_YAML).toContain('# adapters:')
  })
})

describe('createDefaultGlobalConfig', () => {
  let tempDir: string

  beforeEach(() => {
    tempDir = mkdtempSync(join(tmpdir(), 'taskcast-utils-test-'))
  })

  afterEach(() => {
    rmSync(tempDir, { recursive: true, force: true })
  })

  it('creates config file and returns path', () => {
    mockHomedir.mockReturnValue(tempDir)

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})

    const result = createDefaultGlobalConfig()

    expect(result).toBe(join(tempDir, '.taskcast', 'taskcast.config.yaml'))
    expect(existsSync(result!)).toBe(true)
    const content = readFileSync(result!, 'utf-8')
    expect(content).toContain('port: 3721')
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Created default config'))
    logSpy.mockRestore()
  })

  it('returns null and warns when directory creation fails', () => {
    // Point homedir to a file (not a dir) so mkdirSync fails
    const fakePath = join(tempDir, 'blocked-file')
    writeFileSync(fakePath, 'not a dir')
    mockHomedir.mockReturnValue(join(fakePath, 'nested'))

    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})

    const result = createDefaultGlobalConfig()

    expect(result).toBeNull()
    expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('Could not create config'))
    warnSpy.mockRestore()
  })
})

describe('promptCreateGlobalConfig', () => {
  it('returns false when stdin is not a TTY', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: false, configurable: true })

    try {
      const result = await promptCreateGlobalConfig()
      expect(result).toBe(false)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
    }
  })

  it('returns true when user answers Y', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    // When question is called, invoke the callback with 'Y'
    mockRlQuestion.mockImplementation((_msg: string, cb: (answer: string) => void) => {
      cb('Y')
    })

    try {
      const result = await promptCreateGlobalConfig()
      expect(result).toBe(true)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
    }
  })

  it('returns true when user answers empty string (default yes)', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    mockRlQuestion.mockImplementation((_msg: string, cb: (answer: string) => void) => {
      cb('')
    })

    try {
      const result = await promptCreateGlobalConfig()
      expect(result).toBe(true)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
    }
  })

  it('returns false when user answers no', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    mockRlQuestion.mockImplementation((_msg: string, cb: (answer: string) => void) => {
      cb('no')
    })

    try {
      const result = await promptCreateGlobalConfig()
      expect(result).toBe(false)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
    }
  })

  it('returns false when readline closes without answering', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    // Capture the close handler and invoke it
    mockRlOn.mockImplementation((event: string, handler: () => void) => {
      if (event === 'close') {
        // Invoke close handler synchronously before question callback
        setTimeout(() => handler(), 0)
      }
      return { question: mockRlQuestion, close: mockRlClose, on: mockRlOn }
    })
    mockRlQuestion.mockImplementation(() => {
      // Don't call the callback — simulate user closing the terminal
    })

    try {
      const result = await promptCreateGlobalConfig()
      expect(result).toBe(false)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
      // Reset mockRlOn
      mockRlOn.mockReset()
      mockRlOn.mockReturnThis()
    }
  })
})

describe('promptConfirm', () => {
  it('returns false when stdin is not a TTY', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: false, configurable: true })

    try {
      const result = await promptConfirm('Are you sure? ')
      expect(result).toBe(false)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
    }
  })

  it('returns true when user answers yes', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    mockRlQuestion.mockImplementation((_msg: string, cb: (answer: string) => void) => {
      cb('yes')
    })

    try {
      const result = await promptConfirm('Apply changes? ')
      expect(result).toBe(true)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
    }
  })

  it('returns false when user answers n', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    mockRlQuestion.mockImplementation((_msg: string, cb: (answer: string) => void) => {
      cb('n')
    })

    try {
      const result = await promptConfirm('Apply changes? ')
      expect(result).toBe(false)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
    }
  })

  it('returns false when readline closes without answering', async () => {
    const originalIsTTY = process.stdin.isTTY
    Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true })

    mockRlOn.mockImplementation((event: string, handler: () => void) => {
      if (event === 'close') {
        setTimeout(() => handler(), 0)
      }
      return { question: mockRlQuestion, close: mockRlClose, on: mockRlOn }
    })
    mockRlQuestion.mockImplementation(() => {
      // Don't call callback
    })

    try {
      const result = await promptConfirm('Confirm? ')
      expect(result).toBe(false)
    } finally {
      Object.defineProperty(process.stdin, 'isTTY', { value: originalIsTTY, configurable: true })
      mockRlOn.mockReset()
      mockRlOn.mockReturnThis()
    }
  })
})
