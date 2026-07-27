import type { DurableSeriesState, TaskEvent } from './types.js'

const encoder = new TextEncoder()

/**
 * Minimal JSON canonicalization used by the hot/cold storage protocol.
 *
 * Objects are recursively key-sorted and numbers are normalized by numeric
 * value, so digests do not depend on JavaScript object insertion order or on
 * insignificant JSON number formatting.
 */
export function canonicalJson(value: unknown): string {
  if (value === null) return 'null'

  switch (typeof value) {
    case 'boolean':
      return value ? 'true' : 'false'
    case 'number':
      if (!Number.isFinite(value)) throw new TypeError('Archive digest JSON numbers must be finite')
      return String(value)
    case 'string':
      return JSON.stringify(value)
    case 'object':
      if (Array.isArray(value)) {
        const encoded: string[] = []
        for (let index = 0; index < value.length; index++) {
          if (!Object.hasOwn(value, index)) {
            throw new TypeError('Archive digest arrays must not contain holes')
          }
          encoded.push(canonicalJson(value[index]))
        }
        return `[${encoded.join(',')}]`
      }
      if (
        (Object.getPrototypeOf(value) !== Object.prototype &&
          Object.getPrototypeOf(value) !== null) ||
        ('toJSON' in value && typeof value.toJSON === 'function')
      ) {
        throw new TypeError('Archive digest objects must be plain JSON objects')
      }
      return `{${Object.keys(value as Record<string, unknown>)
        .filter((key) => (value as Record<string, unknown>)[key] !== undefined)
        .sort(compareUtf8)
        .map(
          (key) =>
            `${JSON.stringify(key)}:${canonicalJson((value as Record<string, unknown>)[key])}`,
        )
        .join(',')}}`
    default:
      throw new TypeError(`Archive digest cannot encode ${typeof value}`)
  }
}

export function archiveEventRecord(event: TaskEvent): string {
  if (!Number.isSafeInteger(event.index) || event.index < 0) {
    throw new TypeError('Archive digest event index must be a non-negative safe integer')
  }
  if (!Number.isFinite(event.timestamp)) {
    throw new TypeError('Archive digest event timestamp must be finite')
  }
  return canonicalJson([
    'taskcast-event-v1',
    event.id,
    event.taskId,
    String(event.index),
    String(event.timestamp),
    event.type,
    event.level,
    event.data,
    event.seriesId ?? null,
    event.seriesMode ?? null,
    event.seriesAccField ?? null,
  ])
}

export function durableSeriesStateRecord(state: DurableSeriesState): string {
  return canonicalJson([
    'taskcast-series-v1',
    state.taskId,
    state.seriesId,
    state.mode,
    String(state.throughIndex),
    archiveEventRecord(state.event),
  ])
}

export async function sha256Hex(value: string): Promise<string> {
  const digest = await globalThis.crypto.subtle.digest('SHA-256', encoder.encode(value))
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('')
}

export async function computeArchiveBatchDigest(
  previousBatchDigest: string | null,
  events: readonly TaskEvent[],
  seriesLatest: readonly DurableSeriesState[],
): Promise<string> {
  return sha256Hex(
    [
      'taskcast-batch-v1',
      previousBatchDigest ?? '',
      ...events.map(archiveEventRecord),
      ...sortedSeriesRecords(seriesLatest),
    ].join('\n'),
  )
}

export async function computeArchiveSourcePageDigest(
  events: readonly TaskEvent[],
): Promise<string> {
  return sha256Hex(
    [
      'taskcast-source-page-v1',
      ...events.map((event) => canonicalJson([String(event.index), event.id])),
    ].join('\n'),
  )
}

export async function computeArchiveSourceDigest(
  pageDigests: readonly string[],
): Promise<string> {
  return sha256Hex(['taskcast-source-v1', ...pageDigests].join('\n'))
}

export async function computeSeriesStateDigest(
  seriesLatest: readonly DurableSeriesState[],
): Promise<string> {
  return sha256Hex(['taskcast-series-state-v1', ...sortedSeriesRecords(seriesLatest)].join('\n'))
}

function sortedSeriesRecords(seriesLatest: readonly DurableSeriesState[]): string[] {
  return [...seriesLatest]
    .sort((left, right) => {
      const taskOrder = compareUtf8(left.taskId, right.taskId)
      return taskOrder !== 0 ? taskOrder : compareUtf8(left.seriesId, right.seriesId)
    })
    .map(durableSeriesStateRecord)
}

function compareUtf8(left: string, right: string): number {
  const leftBytes = encoder.encode(left)
  const rightBytes = encoder.encode(right)
  const length = Math.min(leftBytes.length, rightBytes.length)
  for (let index = 0; index < length; index++) {
    const difference = leftBytes[index]! - rightBytes[index]!
    if (difference !== 0) return difference
  }
  return leftBytes.length - rightBytes.length
}
