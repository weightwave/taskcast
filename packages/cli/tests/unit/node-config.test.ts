import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mkdtempSync, rmSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import { NodeConfigManager } from '../../src/node-config.js'

describe('NodeConfigManager', () => {
  let tempDir: string

  beforeEach(() => {
    tempDir = mkdtempSync(join(tmpdir(), 'taskcast-node-config-'))
  })

  afterEach(() => {
    rmSync(tempDir, { recursive: true, force: true })
  })

  it('returns default localhost when no nodes configured', () => {
    const mgr = new NodeConfigManager(tempDir)
    const current = mgr.getCurrent()
    expect(current.url).toBe('http://localhost:3721')
    expect(current.token).toBeUndefined()
    expect(current.tokenType).toBeUndefined()
  })

  it('adds and retrieves a node', () => {
    const mgr = new NodeConfigManager(tempDir)
    mgr.add('prod', { url: 'https://tc.example.com', token: 'ey...', tokenType: 'jwt' })
    const node = mgr.get('prod')
    expect(node).toBeDefined()
    expect(node!.url).toBe('https://tc.example.com')
    expect(node!.token).toBe('ey...')
    expect(node!.tokenType).toBe('jwt')
  })

  it('sets and gets current node', () => {
    const mgr = new NodeConfigManager(tempDir)
    mgr.add('prod', { url: 'https://tc.example.com', token: 'tok', tokenType: 'jwt' })
    mgr.use('prod')
    const current = mgr.getCurrent()
    expect(current.url).toBe('https://tc.example.com')
    expect(current.token).toBe('tok')
  })

  it('removes a node', () => {
    const mgr = new NodeConfigManager(tempDir)
    mgr.add('prod', { url: 'https://tc.example.com' })
    mgr.remove('prod')
    expect(mgr.get('prod')).toBeUndefined()
  })

  it('lists all nodes with current marker', () => {
    const mgr = new NodeConfigManager(tempDir)
    mgr.add('local', { url: 'http://localhost:3721' })
    mgr.add('prod', { url: 'https://tc.example.com', token: 'tok', tokenType: 'jwt' })
    mgr.use('prod')
    const list = mgr.list()
    expect(list).toHaveLength(2)

    const localEntry = list.find(n => n.name === 'local')!
    expect(localEntry.current).toBe(false)

    const prodEntry = list.find(n => n.name === 'prod')!
    expect(prodEntry.current).toBe(true)
    expect(prodEntry.url).toBe('https://tc.example.com')
    expect(prodEntry.token).toBe('tok')
    expect(prodEntry.tokenType).toBe('jwt')
  })

  it('throws when using non-existent node', () => {
    const mgr = new NodeConfigManager(tempDir)
    expect(() => mgr.use('ghost')).toThrow()
  })

  it('throws when removing non-existent node', () => {
    const mgr = new NodeConfigManager(tempDir)
    expect(() => mgr.remove('ghost')).toThrow()
  })

  it('resets current to default when current node is removed', () => {
    const mgr = new NodeConfigManager(tempDir)
    mgr.add('prod', { url: 'https://tc.example.com' })
    mgr.use('prod')
    expect(mgr.getCurrent().url).toBe('https://tc.example.com')
    mgr.remove('prod')
    // After removing current, should fall back to default
    const current = mgr.getCurrent()
    expect(current.url).toBe('http://localhost:3721')
  })

  it('persists across instances (write, create new instance, read back)', () => {
    const mgr1 = new NodeConfigManager(tempDir)
    mgr1.add('staging', { url: 'https://s.tc.io', token: 'admin_xxx', tokenType: 'admin' })
    mgr1.use('staging')

    // Create a brand new instance pointing at same config dir
    const mgr2 = new NodeConfigManager(tempDir)
    const current = mgr2.getCurrent()
    expect(current.url).toBe('https://s.tc.io')
    expect(current.token).toBe('admin_xxx')
    expect(current.tokenType).toBe('admin')

    const node = mgr2.get('staging')
    expect(node).toBeDefined()
    expect(node!.url).toBe('https://s.tc.io')
  })
})
