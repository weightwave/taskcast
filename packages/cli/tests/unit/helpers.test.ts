import { describe, it, expect } from 'vitest'
import { parseBooleanEnv } from '../../src/helpers.js'

describe('parseBooleanEnv', () => {
  describe('truthy values', () => {
    it('parses "1" as true', () => {
      expect(parseBooleanEnv('1')).toBe(true)
    })

    it('parses "true" as true', () => {
      expect(parseBooleanEnv('true')).toBe(true)
    })

    it('parses "yes" as true', () => {
      expect(parseBooleanEnv('yes')).toBe(true)
    })

    it('parses "on" as true', () => {
      expect(parseBooleanEnv('on')).toBe(true)
    })

    it('case-insensitive: uppercase TRUE', () => {
      expect(parseBooleanEnv('TRUE')).toBe(true)
    })

    it('case-insensitive: uppercase YES', () => {
      expect(parseBooleanEnv('YES')).toBe(true)
    })

    it('case-insensitive: uppercase ON', () => {
      expect(parseBooleanEnv('ON')).toBe(true)
    })

    it('case-insensitive: mixed case True', () => {
      expect(parseBooleanEnv('True')).toBe(true)
    })

    it('case-insensitive: mixed case Yes', () => {
      expect(parseBooleanEnv('Yes')).toBe(true)
    })

    it('case-insensitive: mixed case On', () => {
      expect(parseBooleanEnv('On')).toBe(true)
    })
  })

  describe('falsy values', () => {
    it('parses "0" as false', () => {
      expect(parseBooleanEnv('0')).toBe(false)
    })

    it('parses "false" as false', () => {
      expect(parseBooleanEnv('false')).toBe(false)
    })

    it('parses "no" as false', () => {
      expect(parseBooleanEnv('no')).toBe(false)
    })

    it('parses "off" as false', () => {
      expect(parseBooleanEnv('off')).toBe(false)
    })

    it('parses empty string as false', () => {
      expect(parseBooleanEnv('')).toBe(false)
    })

    it('parses undefined as false', () => {
      expect(parseBooleanEnv(undefined)).toBe(false)
    })

    it('case-insensitive: uppercase FALSE', () => {
      expect(parseBooleanEnv('FALSE')).toBe(false)
    })

    it('case-insensitive: uppercase NO', () => {
      expect(parseBooleanEnv('NO')).toBe(false)
    })

    it('case-insensitive: uppercase OFF', () => {
      expect(parseBooleanEnv('OFF')).toBe(false)
    })
  })

  describe('whitespace handling', () => {
    it('trims leading whitespace from truthy value', () => {
      expect(parseBooleanEnv('  true')).toBe(true)
    })

    it('trims trailing whitespace from truthy value', () => {
      expect(parseBooleanEnv('true  ')).toBe(true)
    })

    it('trims both leading and trailing whitespace', () => {
      expect(parseBooleanEnv('  true  ')).toBe(true)
    })

    it('trims whitespace around "1"', () => {
      expect(parseBooleanEnv('  1  ')).toBe(true)
    })

    it('trims whitespace around "yes"', () => {
      expect(parseBooleanEnv('  yes  ')).toBe(true)
    })

    it('trims whitespace around "on"', () => {
      expect(parseBooleanEnv('  on  ')).toBe(true)
    })

    it('trims whitespace around falsy values', () => {
      expect(parseBooleanEnv('  false  ')).toBe(false)
    })

    it('only whitespace is falsy', () => {
      expect(parseBooleanEnv('   ')).toBe(false)
    })
  })

  describe('invalid/unknown values', () => {
    it('unknown strings are false', () => {
      expect(parseBooleanEnv('maybe')).toBe(false)
    })

    it('unknown strings with whitespace are false', () => {
      expect(parseBooleanEnv('  maybe  ')).toBe(false)
    })

    it('"2" is false', () => {
      expect(parseBooleanEnv('2')).toBe(false)
    })

    it('"enabled" is false', () => {
      expect(parseBooleanEnv('enabled')).toBe(false)
    })

    it('"disabled" is false', () => {
      expect(parseBooleanEnv('disabled')).toBe(false)
    })
  })
})
