import { describe, it, expect } from 'vitest'
import { writeFileSync, mkdirSync, rmSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import { loadConfigFile } from '@taskcast/core'

describe('CLI — config loading', () => {
  const tmpDir = join(tmpdir(), `taskcast-test-${Date.now()}`)

  it('loads valid YAML config', async () => {
    mkdirSync(tmpDir, { recursive: true })
    const configPath = join(tmpDir, 'config.yaml')
    writeFileSync(configPath, `
port: 4000
auth:
  mode: jwt
adapters:
  broadcast:
    provider: redis
    url: redis://localhost:6379
`)
    const { config } = await loadConfigFile(configPath)
    expect(config.port).toBe(4000)
    expect(config.auth?.mode).toBe('jwt')
    expect(config.adapters?.broadcast?.provider).toBe('redis')
    rmSync(tmpDir, { recursive: true })
  })

  it('returns empty config for nonexistent file', async () => {
    const { config, source } = await loadConfigFile('/tmp/nonexistent-taskcast-config.yaml')
    expect(source).toBe('explicit')
    expect(config).toBeTruthy()
  })

  it('throws on invalid YAML', async () => {
    mkdirSync(tmpDir, { recursive: true })
    const configPath = join(tmpDir, 'bad.yaml')
    writeFileSync(configPath, '{{{{invalid yaml')
    await expect(loadConfigFile(configPath)).rejects.toThrow()
    rmSync(tmpDir, { recursive: true })
  })
})
