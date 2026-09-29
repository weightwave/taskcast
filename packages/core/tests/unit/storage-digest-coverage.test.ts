import { describe, expect, it } from 'vitest'
import { archiveEventRecord, canonicalJson, computeSeriesStateDigest } from '../../src/storage-digest.js'
import type { DurableSeriesState, TaskEvent } from '../../src/types.js'

const event: TaskEvent = {
  id: 'event', taskId: 'task', index: 0, timestamp: 1,
  type: 'progress', level: 'info', data: {},
}

describe('archive digest input integrity', () => {
  it.each([undefined, Symbol('unsupported'), 1n, () => 1])(
    'rejects non-JSON values: %s',
    (value) => expect(() => canonicalJson(value)).toThrow(/cannot encode/),
  )

  it.each([NaN, Infinity, -Infinity])('rejects nonfinite JSON numbers: %s', (value) => {
    expect(() => canonicalJson({ value })).toThrow(/must be finite/)
    expect(() => archiveEventRecord({ ...event, timestamp: value })).toThrow(/timestamp must be finite/)
  })

  it.each([-1, 0.5, Number.MAX_SAFE_INTEGER + 1, NaN])('rejects invalid event indexes: %s', (index) => {
    expect(() => archiveEventRecord({ ...event, index })).toThrow(/non-negative safe integer/)
  })

  it('omits undefined object properties but rejects undefined array entries and custom JSON serializers', () => {
    expect(canonicalJson({ kept: false, omitted: undefined })).toBe('{"kept":false}')
    expect(() => canonicalJson([undefined])).toThrow(/cannot encode undefined/)
    expect(() => canonicalJson({ toJSON: () => 'changed' })).toThrow(/plain JSON objects/)
    expect(canonicalJson(Object.assign(Object.create(null), { a: 1 }))).toBe('{"a":1}')
  })

  it('orders series by UTF-8 task and series identity independently of input order', async () => {
    const series = (taskId: string, seriesId: string): DurableSeriesState => ({
      taskId, seriesId, mode: 'latest', throughIndex: 0,
      event: { ...event, taskId, seriesId, seriesMode: 'latest' },
    })
    const states = [series('task', 'aa'), series('task', 'a'), series('other', 'é'), series('task', '😀')]
    const expected = await computeSeriesStateDigest(states)
    await expect(computeSeriesStateDigest([...states].reverse())).resolves.toBe(expected)
    await expect(computeSeriesStateDigest(states.slice(1))).resolves.not.toBe(expected)
  })
})
