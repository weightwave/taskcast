import { describe, expect, it } from 'vitest'
import type { DurableSeriesState, TaskEvent } from '../../src/types.js'
import {
  archiveEventRecord,
  canonicalJson,
  computeArchiveBatchDigest,
  computeArchiveSourceDigest,
  computeArchiveSourcePageDigest,
  computeSeriesStateDigest,
} from '../../src/storage-digest.js'

const event: TaskEvent = {
  id: 'evt-7',
  taskId: 'task-1',
  index: 7,
  timestamp: 1700000000123,
  type: 'llm.delta',
  level: 'info',
  data: { z: [3, { b: true, a: null }], a: 'hello' },
  seriesId: 'output',
  seriesMode: 'accumulate',
  seriesAccField: 'delta',
}

const series: DurableSeriesState = {
  taskId: 'task-1',
  seriesId: 'output',
  mode: 'accumulate',
  event,
  throughIndex: 7,
}

describe('storage archive digest protocol', () => {
  it('canonicalizes nested JSON independent of object insertion order', () => {
    expect(canonicalJson({ z: 1, a: { y: 2, b: 3 } })).toBe(
      '{"a":{"b":3,"y":2},"z":1}',
    )
    expect(canonicalJson({ a: { b: 3, y: 2 }, z: 1 })).toBe(
      '{"a":{"b":3,"y":2},"z":1}',
    )
    expect(
      canonicalJson([
        1e21,
        1e20,
        1e-7,
        1e-6,
        -0,
        18446744073709551615,
        667082108456853.2,
      ]),
    ).toBe(
      '[1e+21,100000000000000000000,1e-7,0.000001,0,18446744073709552000,667082108456853.2]',
    )
  })

  it('uses the cross-language event encoding fixture', () => {
    expect(archiveEventRecord(event)).toBe(
      '["taskcast-event-v1","evt-7","task-1","7","1700000000123","llm.delta","info",{"a":"hello","z":[3,{"a":null,"b":true}]},"output","accumulate","delta"]',
    )
  })

  it('rejects values whose database JSON representation would differ', () => {
    expect(() => canonicalJson(new Date(0))).toThrow(/plain JSON objects/)
    expect(() => canonicalJson(new Map([['key', 'value']]))).toThrow(/plain JSON objects/)
    expect(() => canonicalJson([, 1])).toThrow(/must not contain holes/)
  })

  it('uses identical SHA-256 fixtures for batches, coverage, and series state', async () => {
    const pageDigest = await computeArchiveSourcePageDigest([event])

    await expect(computeArchiveBatchDigest(null, [event], [series])).resolves.toBe(
      'fcaa595fb88f042f2e86decfa48dd46483f80bd7edb04d5c8b7a5876345003d8',
    )
    expect(pageDigest).toBe('a494e9437592b3a58deb02a98e414ba87cd591079695bb2ebd4dd4c04d506fc8')
    await expect(computeArchiveSourceDigest([pageDigest])).resolves.toBe(
      'd25d5e5dd7d8dba54b03d2bf56156593d5d8394ecbf0a65ce1e3eae8fe1c050a',
    )
    await expect(computeSeriesStateDigest([series])).resolves.toBe(
      '5b58f61debb90051f9a92459b3eb98776cfa02241d7cd9c09fedfa863c70d750',
    )
  })
})
