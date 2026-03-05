import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { writeFileSync, unlinkSync, mkdirSync, rmSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import { interpolateEnvVars, parseConfig, loadConfigFile, resolveAdminToken } from '../../src/config.js'
import type { TaskcastConfig } from '../../src/config.js'

describe('interpolateEnvVars', () => {
  it('replaces ${VAR} with env value', () => {
    process.env['TEST_VAR'] = 'hello'
    expect(interpolateEnvVars('prefix_${TEST_VAR}_suffix')).toBe('prefix_hello_suffix')
  })

  it('leaves ${MISSING} unchanged when var not set', () => {
    delete process.env['MISSING_VAR_XYZ']
    expect(interpolateEnvVars('${MISSING_VAR_XYZ}')).toBe('${MISSING_VAR_XYZ}')
  })

  it('handles multiple vars in same string', () => {
    process.env['CONFIG_A'] = 'foo'
    process.env['CONFIG_B'] = 'bar'
    expect(interpolateEnvVars('${CONFIG_A}:${CONFIG_B}')).toBe('foo:bar')
  })
})

describe('parseConfig - JSON', () => {
  it('parses valid JSON config', () => {
    const json = JSON.stringify({ port: 3721, auth: { mode: 'none' } })
    const config = parseConfig(json, 'json')
    expect(config.port).toBe(3721)
    expect(config.auth?.mode).toBe('none')
  })

  it('coerces string port to number when JSON port is a quoted string', () => {
    const json = JSON.stringify({ port: '8080' })
    const config = parseConfig(json, 'json')
    expect(config.port).toBe(8080)
  })
})

describe('parseConfig - YAML', () => {
  it('parses valid YAML config', () => {
    const yaml = `
port: 3721
auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: my-secret
`
    const config = parseConfig(yaml, 'yaml')
    expect(config.port).toBe(3721)
    expect(config.auth?.mode).toBe('jwt')
    expect(config.auth?.jwt?.secret).toBe('my-secret')
  })

  it('interpolates env vars in YAML values', () => {
    process.env['TEST_PORT'] = '4000'
    const yaml = 'port: ${TEST_PORT}'
    const config = parseConfig(yaml, 'yaml')
    expect(config.port).toBe(4000)
  })

  it('deletes port field when env var interpolates to non-numeric string', () => {
    process.env['BAD_PORT'] = 'notanumber'
    const yaml = 'port: ${BAD_PORT}'
    const config = parseConfig(yaml, 'yaml')
    expect(config.port).toBeUndefined()
  })
})

describe('parseConfig - interpolateObject non-string primitives', () => {
  it('returns numbers and booleans unchanged', () => {
    const json = JSON.stringify({ port: 8080, sentry: { captureTaskFailures: true } })
    const config = parseConfig(json, 'json')
    expect(config.port).toBe(8080)
    expect(config.sentry?.captureTaskFailures).toBe(true)
  })
})

describe('loadConfigFile', () => {
  it('loads a YAML config file from a given path with source "explicit"', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.yaml`)
    writeFileSync(tmpPath, 'port: 9999\n')
    try {
      const result = await loadConfigFile(tmpPath)
      expect(result.config.port).toBe(9999)
      expect(result.source).toBe('explicit')
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('loads a JSON config file from a given path with source "explicit"', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.json`)
    writeFileSync(tmpPath, JSON.stringify({ port: 7777, logLevel: 'debug' }))
    try {
      const result = await loadConfigFile(tmpPath)
      expect(result.config.port).toBe(7777)
      expect(result.config.logLevel).toBe('debug')
      expect(result.source).toBe('explicit')
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('returns source "explicit" with empty config when explicit path does not exist', async () => {
    const result = await loadConfigFile('/tmp/taskcast-nonexistent-xyz-12345.yaml')
    expect(result.config).toEqual({})
    expect(result.source).toBe('explicit')
  })

  it('returns source "none" when no config files exist anywhere', async () => {
    const emptyDir = join(tmpdir(), `taskcast-empty-${Date.now()}`)
    mkdirSync(emptyDir, { recursive: true })
    try {
      const result = await loadConfigFile(undefined, emptyDir)
      expect(result.source).toBe('none')
      expect(result.config).toEqual({})
    } finally {
      rmSync(emptyDir, { recursive: true, force: true })
    }
  })
})

describe('loadConfigFile - global fallback', () => {
  const globalDir = join(tmpdir(), `taskcast-global-test-${Date.now()}`)
  const globalConfigPath = join(globalDir, 'taskcast.config.yaml')

  beforeEach(() => {
    mkdirSync(globalDir, { recursive: true })
  })

  afterEach(() => {
    rmSync(globalDir, { recursive: true, force: true })
  })

  it('finds config in global directory when local directory has none', async () => {
    writeFileSync(globalConfigPath, 'port: 5555\n')
    const result = await loadConfigFile(undefined, globalDir)
    expect(result.config.port).toBe(5555)
    expect(result.source).toBe('global')
  })

  it('does not search global for ts/js/mjs files', async () => {
    writeFileSync(join(globalDir, 'taskcast.config.js'), 'export default { port: 1234 }')
    const result = await loadConfigFile(undefined, globalDir)
    expect(result.source).toBe('none')
  })
})

describe('resolveAdminToken', () => {
  it('auto-generates a ULID token when adminToken is not set', () => {
    const config: TaskcastConfig = {}
    const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})

    const token = resolveAdminToken(config)

    expect(token).toBeDefined()
    expect(typeof token).toBe('string')
    expect(token.length).toBeGreaterThan(0)
    // ULID is 26 characters
    expect(token).toMatch(/^[0-9A-Z]{26}$/)
    // Config should be mutated
    expect(config.adminToken).toBe(token)
    // Should have logged
    expect(consoleSpy).toHaveBeenCalledOnce()
    expect(consoleSpy).toHaveBeenCalledWith(
      `[taskcast] Admin token (auto-generated): ${token}`,
    )

    consoleSpy.mockRestore()
  })

  it('preserves explicitly provided adminToken without logging', () => {
    const config: TaskcastConfig = { adminToken: 'my-secret-token' }
    const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})

    const token = resolveAdminToken(config)

    expect(token).toBe('my-secret-token')
    expect(config.adminToken).toBe('my-secret-token')
    // Should NOT have logged
    expect(consoleSpy).not.toHaveBeenCalled()

    consoleSpy.mockRestore()
  })

  it('generates unique tokens on each call', () => {
    const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})

    const config1: TaskcastConfig = {}
    const config2: TaskcastConfig = {}
    const token1 = resolveAdminToken(config1)
    const token2 = resolveAdminToken(config2)

    expect(token1).not.toBe(token2)

    consoleSpy.mockRestore()
  })

  it('returns existing token on repeated calls to the same config', () => {
    const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})

    const config: TaskcastConfig = {}
    const token1 = resolveAdminToken(config)
    const token2 = resolveAdminToken(config)

    expect(token1).toBe(token2)
    // Should only log once (on the first call)
    expect(consoleSpy).toHaveBeenCalledOnce()

    consoleSpy.mockRestore()
  })

  it('treats empty string adminToken as unset', () => {
    const config: TaskcastConfig = { adminToken: '' }
    const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})

    const token = resolveAdminToken(config)

    // Empty string is falsy, so it should auto-generate
    expect(token).toMatch(/^[0-9A-Z]{26}$/)
    expect(config.adminToken).toBe(token)
    expect(consoleSpy).toHaveBeenCalledOnce()

    consoleSpy.mockRestore()
  })

  it('parses adminToken from JSON config', () => {
    const json = JSON.stringify({ port: 3000, adminToken: 'from-config-file' })
    const config = parseConfig(json, 'json')
    expect(config.adminToken).toBe('from-config-file')
  })

  it('parses adminToken from YAML config', () => {
    const yaml = 'port: 3000\nadminToken: from-yaml-config\n'
    const config = parseConfig(yaml, 'yaml')
    expect(config.adminToken).toBe('from-yaml-config')
  })
})
