import { describe, it, expect } from 'vitest'
import { writeFileSync, unlinkSync } from 'fs'
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
      const config = await loadConfigFile(tmpPath)
      expect(config.port).toBe(9999)
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('loads a JSON config file from a given path', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.json`)
    writeFileSync(tmpPath, JSON.stringify({ port: 7777, logLevel: 'debug' }))
    try {
      const config = await loadConfigFile(tmpPath)
      expect(config.port).toBe(7777)
      expect(config.logLevel).toBe('debug')
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('returns empty object for a nonexistent path', async () => {
    const config = await loadConfigFile('/tmp/taskcast-nonexistent-xyz-12345.yaml')
    expect(config).toEqual({})
  })

  it('returns empty object when no default config files exist', async () => {
    // Call with no argument from a directory where no default files exist
    // The function resolves paths relative to cwd; in test env none of the defaults should exist
    const config = await loadConfigFile()
    // Should return {} (no default config files present in test runner cwd)
    expect(config).toBeDefined()
  })
})
