import { describe, it, expect } from 'vitest'
import { resolvePostgresUrl, formatDisplayUrl } from '../../src/migrate-helpers.js'

describe('resolvePostgresUrl', () => {
  it('prefers --url flag over all', () => {
    expect(resolvePostgresUrl({
      url: 'postgres://flag',
      envUrl: 'postgres://env',
      configUrl: 'postgres://config',
    })).toBe('postgres://flag')
  })

  it('falls back to env var', () => {
    expect(resolvePostgresUrl({
      envUrl: 'postgres://env',
      configUrl: 'postgres://config',
    })).toBe('postgres://env')
  })

  it('falls back to config', () => {
    expect(resolvePostgresUrl({
      configUrl: 'postgres://config',
    })).toBe('postgres://config')
  })

  it('returns undefined when no URL', () => {
    expect(resolvePostgresUrl({})).toBeUndefined()
  })
})

describe('formatDisplayUrl', () => {
  it('formats standard postgres URL', () => {
    expect(formatDisplayUrl('postgres://user:pass@myhost:5433/mydb'))
      .toBe('myhost:5433/mydb')
  })

  it('uses default port 5432', () => {
    expect(formatDisplayUrl('postgres://user@myhost/mydb'))
      .toBe('myhost:5432/mydb')
  })

  it('defaults db name to postgres', () => {
    expect(formatDisplayUrl('postgres://user@myhost:5432'))
      .toBe('myhost:5432/postgres')
  })

  it('returns raw string for invalid URL', () => {
    expect(formatDisplayUrl('not-a-url')).toBe('not-a-url')
  })
})
