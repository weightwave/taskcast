import { describe, it, expect } from 'vitest'
import { formatNodeList } from '../../src/commands/node.js'
import type { NodeListEntry } from '../../src/node-config.js'

describe('formatNodeList', () => {
  it('shows current marker (*) for current node', () => {
    const nodes: NodeListEntry[] = [
      { name: 'local', url: 'http://localhost:3721', current: false },
      { name: 'prod', url: 'https://tc.example.com', token: 'ey...', tokenType: 'jwt', current: true },
    ]
    const output = formatNodeList(nodes)
    expect(output).toContain('* prod')
    expect(output).toContain('  local')
    // 'local' line should NOT have the * marker
    const lines = output.split('\n')
    const localLine = lines.find(l => l.includes('local'))!
    expect(localLine.trimStart().startsWith('*')).toBe(false)
  })

  it('shows empty message when no nodes configured', () => {
    const output = formatNodeList([])
    expect(output).toContain('No nodes configured')
    expect(output).toContain('http://localhost:3721')
  })

  it('shows token type in parentheses', () => {
    const nodes: NodeListEntry[] = [
      { name: 'prod', url: 'https://tc.example.com', token: 'ey...', tokenType: 'jwt', current: true },
      { name: 'staging', url: 'https://s.tc.io', token: 'admin_xxx', tokenType: 'admin', current: false },
      { name: 'local', url: 'http://localhost:3721', current: false },
    ]
    const output = formatNodeList(nodes)
    expect(output).toContain('(jwt)')
    expect(output).toContain('(admin)')
    // local has no token type, so no parentheses
    const lines = output.split('\n')
    const localLine = lines.find(l => l.includes('local'))!
    expect(localLine).not.toContain('(')
  })
})
