import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { writeFileSync, unlinkSync, mkdirSync, rmSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import { interpolateEnvVars, parseConfig, loadConfigFile } from '../../src/config.js'

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
  it('loads a YAML config file from a given path', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.yaml`)
    writeFileSync(tmpPath, 'port: 9999\n')
    try {
      const { config } = await loadConfigFile(tmpPath)
      expect(config.port).toBe(9999)
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('loads a JSON config file from a given path', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.json`)
    writeFileSync(tmpPath, JSON.stringify({ port: 7777, logLevel: 'debug' }))
    try {
      const { config } = await loadConfigFile(tmpPath)
      expect(config.port).toBe(7777)
      expect(config.logLevel).toBe('debug')
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('returns empty config for a nonexistent explicit path', async () => {
    const { config } = await loadConfigFile('/tmp/taskcast-nonexistent-xyz-12345.yaml')
    expect(config).toEqual({})
  })

  it('returns a defined result when no default config files exist', async () => {
    const result = await loadConfigFile()
    expect(result.config).toBeDefined()
  })
})

describe('loadConfigFile - return type with source', () => {
  it('returns source "explicit" when a path is given and file exists', async () => {
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

  it('returns source "explicit" with empty config when explicit path does not exist', async () => {
    const result = await loadConfigFile('/tmp/taskcast-nonexistent-xyz-12345.yaml')
    expect(result.config).toEqual({})
    expect(result.source).toBe('explicit')
  })

  it('returns source "none" when no config files exist anywhere', async () => {
    const result = await loadConfigFile()
    expect(result.source).toBe('none')
    expect(result.config).toEqual({})
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
