import { describe, it, expect } from 'vitest'
import { parseMigrationFilename, computeChecksum } from '../../src/migration-runner.js'

describe('parseMigrationFilename', () => {
  it('parses a standard filename', () => {
    const result = parseMigrationFilename('001_initial.sql')
    expect(result).toEqual({ version: 1, description: 'initial' })
  })

  it('parses a multi-word description', () => {
    const result = parseMigrationFilename('002_add_worker_tables.sql')
    expect(result).toEqual({ version: 2, description: 'add worker tables' })
  })

  it('parses large version numbers', () => {
    const result = parseMigrationFilename('100_some_migration.sql')
    expect(result).toEqual({ version: 100, description: 'some migration' })
  })

  it('returns null for non-sql files', () => {
    expect(parseMigrationFilename('001_initial.txt')).toBeNull()
    expect(parseMigrationFilename('001_initial.js')).toBeNull()
    expect(parseMigrationFilename('readme.md')).toBeNull()
  })

  it('returns null for files without version prefix', () => {
    expect(parseMigrationFilename('initial.sql')).toBeNull()
    expect(parseMigrationFilename('abc_initial.sql')).toBeNull()
    expect(parseMigrationFilename('_initial.sql')).toBeNull()
  })

  it('returns null for files without description', () => {
    expect(parseMigrationFilename('001.sql')).toBeNull()
  })

  it('returns null for files with empty description after underscore', () => {
    expect(parseMigrationFilename('001_.sql')).toBeNull()
  })

  it('strips leading zeros from version', () => {
    const result = parseMigrationFilename('007_bond.sql')
    expect(result?.version).toBe(7)
  })
})

describe('computeChecksum', () => {
  it('returns a Buffer of length 48 (SHA-384)', () => {
    const checksum = computeChecksum('SELECT 1;')
    expect(Buffer.isBuffer(checksum)).toBe(true)
    expect(checksum.length).toBe(48)
  })

  it('returns consistent results for the same input', () => {
    const a = computeChecksum('CREATE TABLE foo (id INT);')
    const b = computeChecksum('CREATE TABLE foo (id INT);')
    expect(a.equals(b)).toBe(true)
  })

  it('returns different results for different inputs', () => {
    const a = computeChecksum('CREATE TABLE foo (id INT);')
    const b = computeChecksum('CREATE TABLE bar (id INT);')
    expect(a.equals(b)).toBe(false)
  })

  it('handles empty string', () => {
    const checksum = computeChecksum('')
    expect(checksum.length).toBe(48)
  })

  it('handles multi-line SQL', () => {
    const sql = 'CREATE TABLE foo (\n  id INT PRIMARY KEY,\n  name TEXT\n);'
    const checksum = computeChecksum(sql)
    expect(checksum.length).toBe(48)
  })
})
