import { describe, it, expect } from 'vitest'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { parseMigrationFilename, computeChecksum, loadMigrationFiles } from '../../src/migration-runner.js'

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

describe('loadMigrationFiles', () => {
  it('loads and sorts SQL files from directory', () => {
    const dir = mkdtempSync(join(tmpdir(), 'migrations-'))
    writeFileSync(join(dir, '002_second.sql'), 'SELECT 2;')
    writeFileSync(join(dir, '001_first.sql'), 'SELECT 1;')
    writeFileSync(join(dir, 'README.md'), 'not a migration')

    const files = loadMigrationFiles(dir)
    expect(files).toHaveLength(2)
    expect(files[0]!.version).toBe(1)
    expect(files[0]!.filename).toBe('001_first.sql')
    expect(files[0]!.sql).toBe('SELECT 1;')
    expect(files[0]!.description).toBe('first')
    expect(files[0]!.checksum).toBeInstanceOf(Buffer)
    expect(files[1]!.version).toBe(2)
    expect(files[1]!.filename).toBe('002_second.sql')
  })

  it('returns empty array for empty directory', () => {
    const dir = mkdtempSync(join(tmpdir(), 'migrations-'))
    expect(loadMigrationFiles(dir)).toEqual([])
  })

  it('skips files with invalid names', () => {
    const dir = mkdtempSync(join(tmpdir(), 'migrations-'))
    writeFileSync(join(dir, 'not_versioned.sql'), 'SELECT 1;')
    writeFileSync(join(dir, '001_valid.sql'), 'SELECT 1;')
    writeFileSync(join(dir, 'readme.md'), 'not sql')

    const files = loadMigrationFiles(dir)
    expect(files).toHaveLength(1)
    expect(files[0]!.filename).toBe('001_valid.sql')
  })
})
