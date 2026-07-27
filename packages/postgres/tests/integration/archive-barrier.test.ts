import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { join } from 'node:path'
import type {
  ArchiveBatch,
  ArchiveGeneration,
  ArchiveSourceManifest,
  DurableSeriesState,
  Task,
  TaskEvent,
} from '@taskcast/core'
import {
  computeArchiveBatchDigest,
  computeArchiveSourceDigest,
  computeArchiveSourcePageDigest,
  computeSeriesStateDigest,
} from '@taskcast/core'
import { PostgresLongTermStore } from '../../src/long-term.js'
import { runMigrations } from '../../src/migration-runner.js'

let container: StartedTestContainer
let sql: ReturnType<typeof postgres>
let store: PostgresLongTermStore

beforeAll(async () => {
  let connection = process.env['TASKCAST_TEST_POSTGRES_URL']
  if (!connection) {
    container = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'testdb',
      })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()
    connection = `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`
  }
  sql = postgres(connection, { onnotice: () => {} })
  await runMigrations(sql, join(import.meta.dirname, '../../../../migrations/postgres'))
  store = new PostgresLongTermStore(sql)
}, 120000)

afterAll(async () => {
  await sql?.end()
  await container?.stop()
})

beforeEach(async () => {
  await sql`TRUNCATE taskcast_tasks CASCADE`
})

const makeTask = (id = 'task-archive'): Task => ({
  id,
  status: 'running',
  createdAt: 1000,
  updatedAt: 2000,
})

const makeEvent = (index: number, overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: `event-${index}`,
  taskId: 'task-archive',
  index,
  timestamp: 1000 + index,
  type: 'llm.delta',
  level: 'info',
  data: { text: `chunk-${index}` },
  ...overrides,
})

async function startRelease(generation = 'generation-1'): Promise<void> {
  await store.saveTask(makeTask())
  const metadata = await store.getTaskStorageMetadata('task-archive')
  expect(metadata).not.toBeNull()
  await expect(
    store.compareAndSetTaskStorageMetadata({
      taskId: 'task-archive',
      expectedStorageState: 'hot',
      expectedStorageEpoch: 1,
      expectedReleaseGeneration: null,
      next: {
        ...metadata!,
        storageState: 'releasing',
        activeReleaseGeneration: generation,
      },
    }),
  ).resolves.toBe(true)
}

async function buildArchive(
  events: TaskEvent[],
  seriesLatest: DurableSeriesState[] = [],
  generation = 'generation-1',
  batchSize = 1,
  manifestOverrides: Partial<ArchiveSourceManifest> = {},
): Promise<{ generation: ArchiveGeneration; batches: ArchiveBatch[] }> {
  const pages: TaskEvent[][] = []
  for (let offset = 0; offset < events.length; offset += batchSize) {
    pages.push(events.slice(offset, offset + batchSize))
  }
  const pageDigests = await Promise.all(pages.map(computeArchiveSourcePageDigest))
  const targetWatermark = events.at(-1)?.index ?? -1
  const manifest: ArchiveSourceManifest = {
    priorWatermark: -1,
    targetWatermark,
    sourceEntryCount: events.length,
    sourceDigest: await computeArchiveSourceDigest(pageDigests),
    seriesStateDigest: await computeSeriesStateDigest(seriesLatest),
    expectedBatchOrdinals: pages.map((_, ordinal) => ordinal),
    ...manifestOverrides,
  }
  const archiveGeneration: ArchiveGeneration = {
    taskId: 'task-archive',
    generation,
    storageEpoch: 1,
    targetWatermark,
    manifest,
    status: 'open',
    createdAt: 3000,
    updatedAt: 3000,
  }
  const batches: ArchiveBatch[] = []
  let previousBatchDigest: string | null = null
  for (let ordinal = 0; ordinal < pages.length; ordinal++) {
    const page = pages[ordinal]!
    const pageSeries: DurableSeriesState[] = []
    const batchDigest = await computeArchiveBatchDigest(
      previousBatchDigest,
      page,
      pageSeries,
    )
    batches.push({
      receipt: {
        taskId: 'task-archive',
        generation,
        ordinal,
        previousBatchDigest,
        batchDigest,
        entryCount: page.length,
        firstIndex: page[0]?.index ?? null,
        lastIndex: page.at(-1)?.index ?? null,
      },
      events: page,
      seriesLatest: pageSeries,
    })
    previousBatchDigest = batchDigest
  }
  return { generation: archiveGeneration, batches }
}

describe('PostgresLongTermStore archive barrier', () => {
  it('rejects non-monotonic or internally inconsistent metadata CAS updates', async () => {
    await store.saveTask(makeTask())
    const metadata = (await store.getTaskStorageMetadata('task-archive'))!

    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'hot',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: null,
        next: { ...metadata, storageEpoch: 0 },
      }),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })

    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'hot',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: null,
        next: { ...metadata, activeReleaseGeneration: 'generation-without-release' },
      }),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })

    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'hot',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: null,
        next: { ...metadata, archiveWatermark: 0 },
      }),
    ).resolves.toBe(false)
  })

  it('finalizes an empty source without inventing a batch', async () => {
    await startRelease()
    const archive = await buildArchive([])

    await store.beginArchive(archive.generation)
    await expect(
      store.finalizeArchive('task-archive', 'generation-1', makeTask(), []),
    ).resolves.toBe(-1)
    await expect(store.getEvents('task-archive')).resolves.toEqual([])
  })

  it('rejects an empty source that advances the archive watermark', async () => {
    await startRelease()
    const archive = await buildArchive([], [], 'generation-1', 1, {
      targetWatermark: 0,
    })
    archive.generation.targetWatermark = 0

    await expect(store.beginArchive(archive.generation)).rejects.toMatchObject({
      code: 'storage_integrity_error',
    })
  })

  it('allows a cold rehydration generation but rejects one on a hot task', async () => {
    await store.saveTask(makeTask())
    const metadata = (await store.getTaskStorageMetadata('task-archive'))!

    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'hot',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: null,
        next: {
          ...metadata,
          storageState: 'cold',
          activeReleaseGeneration: 'rehydration-generation',
        },
      }),
    ).resolves.toBe(true)
  })

  it('rejects compact source events without bounded final series coverage', async () => {
    await startRelease()
    const compactEvent = makeEvent(0, {
      seriesId: 'output',
      seriesMode: 'latest',
    })
    const missing = await buildArchive([compactEvent])
    await store.beginArchive(missing.generation)
    await store.archiveBatch('task-archive', 'generation-1', missing.batches[0]!)
    await expect(
      store.finalizeArchive('task-archive', 'generation-1', makeTask(), []),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })

    await sql`TRUNCATE taskcast_tasks CASCADE`
    await startRelease()
    const outOfBoundsState: DurableSeriesState = {
      taskId: 'task-archive',
      seriesId: 'output',
      mode: 'latest',
      event: compactEvent,
      throughIndex: 1,
    }
    const outOfBounds = await buildArchive([compactEvent], [outOfBoundsState])
    await store.beginArchive(outOfBounds.generation)
    await store.archiveBatch('task-archive', 'generation-1', outOfBounds.batches[0]!)
    await expect(
      store.finalizeArchive(
        'task-archive',
        'generation-1',
        makeTask(),
        [outOfBoundsState],
      ),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
  })

  it('does not hide a compact event identity conflict by deleting the old row', async () => {
    await startRelease()
    const oldEvent = makeEvent(0, {
      data: { text: 'old' },
      seriesId: 'output',
      seriesMode: 'latest',
    })
    await store.saveEvent(oldEvent)
    const replacement = makeEvent(0, {
      data: { text: 'replacement' },
      seriesId: 'output',
      seriesMode: 'latest',
    })
    const state: DurableSeriesState = {
      taskId: 'task-archive',
      seriesId: 'output',
      mode: 'latest',
      event: replacement,
      throughIndex: 0,
    }
    const archive = await buildArchive([replacement], [state])
    await store.beginArchive(archive.generation)
    await store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!)

    await expect(
      store.finalizeArchive('task-archive', 'generation-1', makeTask(), [state]),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await expect(store.getEvents('task-archive')).resolves.toEqual([oldEvent])
  })

  it('finalizes a complete manifest and safely replays lost responses', async () => {
    await startRelease()
    const seriesEvent = makeEvent(1, {
      id: 'series-event',
      data: { delta: 'hello world' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    const seriesLatest: DurableSeriesState[] = [
      {
        taskId: 'task-archive',
        seriesId: 'output',
        mode: 'accumulate',
        event: seriesEvent,
        throughIndex: 1,
      },
    ]
    const archive = await buildArchive([makeEvent(0), seriesEvent], seriesLatest)

    await expect(store.beginArchive(archive.generation)).resolves.toEqual(archive.generation)
    for (const batch of archive.batches) {
      await expect(
        store.archiveBatch('task-archive', 'generation-1', batch),
      ).resolves.toEqual(batch.receipt)
    }
    await expect(
      store.finalizeArchive('task-archive', 'generation-1', makeTask(), seriesLatest),
    ).resolves.toBe(1)

    await expect(store.beginArchive(archive.generation)).resolves.toMatchObject({
      status: 'finalized',
    })
    await expect(
      store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!),
    ).resolves.toEqual(archive.batches[0]!.receipt)
    await expect(
      store.finalizeArchive('task-archive', 'generation-1', makeTask(), seriesLatest),
    ).resolves.toBe(1)
    await expect(store.getArchiveWatermark('task-archive')).resolves.toBe(1)
    await expect(store.getLastEventIndex('task-archive')).resolves.toBe(1)
    await expect(store.getDurableSeriesState('task-archive')).resolves.toEqual(seriesLatest)
    await expect(store.getEvents('task-archive')).resolves.toEqual([
      makeEvent(0),
      seriesEvent,
    ])
  })

  it('keeps compact coverage bounded across accumulate batches', async () => {
    await startRelease()
    const first = makeEvent(0, {
      data: { delta: 'hello' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    const second = makeEvent(1, {
      data: { delta: ' world' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    const finalEvent = {
      ...second,
      data: { delta: 'hello world' },
    }
    const finalState: DurableSeriesState = {
      taskId: 'task-archive',
      seriesId: 'output',
      mode: 'accumulate',
      event: finalEvent,
      throughIndex: 1,
    }
    await store.accumulateSeries('task-archive', 'output', first, 'delta')
    await store.accumulateSeries('task-archive', 'output', second, 'delta')
    const archive = await buildArchive([first, second], [finalState])
    await store.beginArchive(archive.generation)
    for (const batch of archive.batches) {
      expect(batch.seriesLatest).toEqual([])
      await store.archiveBatch('task-archive', 'generation-1', batch)
    }

    const receipts = await sql`
      SELECT series_coverage
      FROM taskcast_archive_batches
      WHERE task_id = 'task-archive' AND generation = 'generation-1'
      ORDER BY ordinal
    `
    expect(receipts.map((row) => row['series_coverage'])).toEqual([
      [{ seriesId: 'output', mode: 'accumulate', throughIndex: 0 }],
      [{ seriesId: 'output', mode: 'accumulate', throughIndex: 1 }],
    ])

    await expect(
      store.finalizeArchive(
        'task-archive',
        'generation-1',
        makeTask(),
        [finalState],
      ),
    ).resolves.toBe(1)
    await expect(store.getEvents('task-archive')).resolves.toEqual([finalEvent])
    await expect(
      store.accumulateSeries('task-archive', 'output', second, 'delta'),
    ).resolves.toEqual(finalEvent)
    await expect(store.getEvents('task-archive')).resolves.toEqual([finalEvent])
  })

  it('finalizes caught-up latest state and ignores a delayed covered write', async () => {
    await startRelease()
    const first = makeEvent(0, {
      data: { status: 'starting' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    const second = makeEvent(1, {
      data: { status: 'ready' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    await store.replaceLastSeriesEvent('task-archive', 'status', first)
    await store.replaceLastSeriesEvent('task-archive', 'status', second)
    const finalState: DurableSeriesState = {
      taskId: 'task-archive',
      seriesId: 'status',
      mode: 'latest',
      event: second,
      throughIndex: 1,
    }
    const archive = await buildArchive([first, second], [finalState])
    await store.beginArchive(archive.generation)
    for (const batch of archive.batches) {
      await store.archiveBatch('task-archive', 'generation-1', batch)
    }
    await expect(
      store.finalizeArchive(
        'task-archive',
        'generation-1',
        makeTask(),
        [finalState],
      ),
    ).resolves.toBe(1)

    await store.replaceLastSeriesEvent('task-archive', 'status', first)
    await expect(store.getEvents('task-archive')).resolves.toEqual([second])
  })

  it('stores compact coverage in the shared UTF-8 series order', async () => {
    await startRelease()
    const bmp = makeEvent(1, {
      seriesId: '\uE000',
      seriesMode: 'latest',
    })
    const supplementary = makeEvent(0, {
      seriesId: '\u{10000}',
      seriesMode: 'latest',
    })
    const archive = await buildArchive([supplementary, bmp], [], 'generation-1', 2)
    await store.beginArchive(archive.generation)
    await store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!)

    const [receipt] = await sql`
      SELECT series_coverage
      FROM taskcast_archive_batches
      WHERE task_id = 'task-archive' AND generation = 'generation-1'
    `
    expect(
      (receipt!['series_coverage'] as Array<{ seriesId: string }>).map(
        (entry) => entry.seriesId,
      ),
    ).toEqual(['\uE000', '\u{10000}'])
  })

  it('rejects final state that omits a pre-migration compact event row', async () => {
    await startRelease()
    await store.saveEvent(
      makeEvent(0, {
        seriesId: 'legacy-output',
        seriesMode: 'latest',
      }),
    )
    const archive = await buildArchive([])
    await store.beginArchive(archive.generation)

    await expect(
      store.finalizeArchive('task-archive', 'generation-1', makeTask(), []),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
  })

  it('does not regress a newer pre-migration compact event row', async () => {
    await startRelease()
    const legacyNewer = makeEvent(1, {
      seriesId: 'legacy-output',
      seriesMode: 'latest',
    })
    await store.saveEvent(legacyNewer)
    const source = makeEvent(0, {
      seriesId: 'legacy-output',
      seriesMode: 'latest',
    })
    const finalState: DurableSeriesState = {
      taskId: 'task-archive',
      seriesId: 'legacy-output',
      mode: 'latest',
      event: source,
      throughIndex: 0,
    }
    const archive = await buildArchive([source], [finalState])
    await store.beginArchive(archive.generation)
    await store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!)

    await expect(
      store.finalizeArchive(
        'task-archive',
        'generation-1',
        makeTask(),
        [finalState],
      ),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await expect(store.getEvents('task-archive')).resolves.toEqual([legacyNewer])
  })

  it('rejects a conflicting begin replay', async () => {
    await startRelease()
    const archive = await buildArchive([makeEvent(0)])
    await store.beginArchive(archive.generation)

    await expect(
      store.beginArchive({
        ...archive.generation,
        manifest: { ...archive.generation.manifest, sourceEntryCount: 2 },
      }),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
  })

  it('rejects missing, reordered, and broken batch chains', async () => {
    await startRelease()
    const archive = await buildArchive([makeEvent(0), makeEvent(1)])
    await store.beginArchive(archive.generation)

    await expect(
      store.archiveBatch('task-archive', 'generation-1', archive.batches[1]!),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!)

    const broken = structuredClone(archive.batches[1]!)
    broken.receipt.previousBatchDigest = 'f'.repeat(64)
    broken.receipt.batchDigest = await computeArchiveBatchDigest(
      broken.receipt.previousBatchDigest,
      broken.events,
      broken.seriesLatest,
    )
    await expect(
      store.archiveBatch('task-archive', 'generation-1', broken),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })

    const overlapping = structuredClone(archive.batches[1]!)
    overlapping.events = [makeEvent(0, { id: 'overlapping-event' })]
    overlapping.receipt.firstIndex = 0
    overlapping.receipt.lastIndex = 0
    overlapping.receipt.batchDigest = await computeArchiveBatchDigest(
      overlapping.receipt.previousBatchDigest,
      overlapping.events,
      overlapping.seriesLatest,
    )
    await expect(
      store.archiveBatch('task-archive', 'generation-1', overlapping),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })

    await store.archiveBatch('task-archive', 'generation-1', archive.batches[1]!)
    await sql`
      DELETE FROM taskcast_archive_batches
      WHERE task_id = 'task-archive' AND generation = 'generation-1' AND ordinal = 0
    `
    await expect(
      store.finalizeArchive('task-archive', 'generation-1', makeTask(), []),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
  })

  it('rejects changed content for an idempotent batch receipt', async () => {
    await startRelease()
    const archive = await buildArchive([makeEvent(0)])
    await store.beginArchive(archive.generation)
    await store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!)

    const changed = structuredClone(archive.batches[0]!)
    changed.events[0]!.data = { text: 'tampered' }
    await expect(
      store.archiveBatch('task-archive', 'generation-1', changed),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })

    const presentationOnly = structuredClone(archive.batches[0]!)
    presentationOnly.events[0]!.seriesSnapshot = true
    await expect(
      store.archiveBatch('task-archive', 'generation-1', presentationOnly),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })

    const nonJson = structuredClone(archive.batches[0]!)
    nonJson.events[0]!.data = undefined
    await expect(
      store.archiveBatch('task-archive', 'generation-1', nonJson),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
  })

  it('rejects an event ID/index conflict instead of overwriting canonical history', async () => {
    await startRelease()
    await store.saveEvent({ ...makeEvent(0), id: 'different-event', data: { text: 'old' } })
    const archive = await buildArchive([makeEvent(0)])
    await store.beginArchive(archive.generation)

    await expect(
      store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await expect(store.getEvents('task-archive')).resolves.toEqual([
      { ...makeEvent(0), id: 'different-event', data: { text: 'old' } },
    ])
  })

  it('allows only one canonical payload when stale generations race', async () => {
    await startRelease('generation-a')
    const first = await buildArchive([makeEvent(0)], [], 'generation-a')
    await store.beginArchive(first.generation)

    const metadata = (await store.getTaskStorageMetadata('task-archive'))!
    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'releasing',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: 'generation-a',
        next: {
          ...metadata,
          activeReleaseGeneration: 'generation-b',
        },
      }),
    ).resolves.toBe(true)
    const second = await buildArchive(
      [makeEvent(0, { data: { text: 'conflicting' } })],
      [],
      'generation-b',
    )
    await store.beginArchive(second.generation)

    const results = await Promise.allSettled([
      store.archiveBatch('task-archive', 'generation-a', first.batches[0]!),
      store.archiveBatch('task-archive', 'generation-b', second.batches[0]!),
    ])
    expect(results.filter((result) => result.status === 'fulfilled')).toHaveLength(1)
    expect(results.filter((result) => result.status === 'rejected')).toHaveLength(1)
    const [{ count }] = await sql`
      SELECT COUNT(*)::int AS count FROM taskcast_archive_batches
      WHERE task_id = 'task-archive'
    `
    expect(count).toBe(1)
  })

  it('rejects a stale keep-all row that conflicts with compact source coverage', async () => {
    await startRelease('generation-a')
    const stale = await buildArchive([makeEvent(0)], [], 'generation-a')
    await store.beginArchive(stale.generation)
    await store.archiveBatch('task-archive', 'generation-a', stale.batches[0]!)

    const metadata = (await store.getTaskStorageMetadata('task-archive'))!
    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'releasing',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: 'generation-a',
        next: {
          ...metadata,
          activeReleaseGeneration: 'generation-b',
        },
      }),
    ).resolves.toBe(true)

    const first = makeEvent(0, {
      seriesId: 'status',
      seriesMode: 'latest',
    })
    const second = makeEvent(1, {
      seriesId: 'status',
      seriesMode: 'latest',
    })
    const finalState: DurableSeriesState = {
      taskId: 'task-archive',
      seriesId: 'status',
      mode: 'latest',
      event: second,
      throughIndex: 1,
    }
    const active = await buildArchive([first, second], [finalState], 'generation-b')
    await store.beginArchive(active.generation)

    await expect(
      store.archiveBatch('task-archive', 'generation-b', active.batches[0]!),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await expect(store.getArchiveWatermark('task-archive')).resolves.toBe(-1)
  })

  it('rejects wrong source count, coverage digest, and series digest at finalize', async () => {
    for (const manifestOverrides of [
      { sourceEntryCount: 2 },
      { sourceDigest: '0'.repeat(64) },
      { seriesStateDigest: '0'.repeat(64) },
    ]) {
      await sql`TRUNCATE taskcast_tasks CASCADE`
      await startRelease()
      const archive = await buildArchive(
        [makeEvent(0)],
        [],
        'generation-1',
        1,
        manifestOverrides,
      )
      await store.beginArchive(archive.generation)
      await store.archiveBatch('task-archive', 'generation-1', archive.batches[0]!)
      await expect(
        store.finalizeArchive('task-archive', 'generation-1', makeTask(), []),
      ).rejects.toMatchObject({ code: 'storage_integrity_error' })
      await expect(store.getArchiveWatermark('task-archive')).resolves.toBe(-1)
    }
  })

  it('rejects a generation that does not own the active task fence', async () => {
    await startRelease('different-generation')
    const archive = await buildArchive([makeEvent(0)])

    await expect(store.beginArchive(archive.generation)).rejects.toMatchObject({
      code: 'storage_integrity_error',
    })
  })

  it('keeps the archive watermark monotonic across generations and metadata CAS', async () => {
    await startRelease('generation-1')
    const first = await buildArchive([makeEvent(0)], [], 'generation-1')
    await store.beginArchive(first.generation)
    await store.archiveBatch('task-archive', 'generation-1', first.batches[0]!)
    await store.finalizeArchive('task-archive', 'generation-1', makeTask(), [])

    let metadata = (await store.getTaskStorageMetadata('task-archive'))!
    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'releasing',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: 'generation-1',
        next: {
          ...metadata,
          storageState: 'hot',
          activeReleaseGeneration: null,
        },
      }),
    ).resolves.toBe(true)
    metadata = (await store.getTaskStorageMetadata('task-archive'))!
    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'hot',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: null,
        next: {
          ...metadata,
          storageState: 'releasing',
          activeReleaseGeneration: 'generation-2',
        },
      }),
    ).resolves.toBe(true)

    const second = await buildArchive(
      [makeEvent(1)],
      [],
      'generation-2',
      1,
      { priorWatermark: 0 },
    )
    await store.beginArchive(second.generation)
    await store.archiveBatch('task-archive', 'generation-2', second.batches[0]!)
    await expect(
      store.finalizeArchive('task-archive', 'generation-2', makeTask(), []),
    ).resolves.toBe(1)

    metadata = (await store.getTaskStorageMetadata('task-archive'))!
    await expect(
      store.compareAndSetTaskStorageMetadata({
        taskId: 'task-archive',
        expectedStorageState: 'releasing',
        expectedStorageEpoch: 1,
        expectedReleaseGeneration: 'generation-2',
        next: { ...metadata, archiveWatermark: 0 },
      }),
    ).resolves.toBe(false)
    await expect(store.getArchiveWatermark('task-archive')).resolves.toBe(1)
  })
})
