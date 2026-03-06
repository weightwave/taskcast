import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { createWorkersRouter } from '../../src/routes/workers.js'
import type { AuthContext } from '../../src/auth.js'

function makeApp(authOverride?: Partial<AuthContext>) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    c.set('auth', { taskIds: '*', scope: ['*'], ...authOverride } as AuthContext)
    await next()
  })
  app.route('/workers', createWorkersRouter(manager, engine))
  return { app, engine, manager, store }
}

function patchStatus(app: Hono, workerId: string, status: string) {
  return app.request(`/workers/${workerId}/status`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ status }),
  })
}

describe('PATCH /workers/:workerId/status', () => {
  it('sets worker to draining', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const res = await patchStatus(app, worker.id, 'draining')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe(worker.id)
    expect(body.status).toBe('draining')
  })

  it('resumes worker from draining to idle', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // First drain
    await patchStatus(app, worker.id, 'draining')
    // Then resume
    const res = await patchStatus(app, worker.id, 'idle')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('idle')
  })

  it('rejects invalid status values', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const res = await patchStatus(app, worker.id, 'busy')
    expect(res.status).toBe(400)
  })

  it('rejects completely invalid status string', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const res = await patchStatus(app, worker.id, 'nonsense')
    expect(res.status).toBe(400)
  })

  it('returns 404 when worker not found', async () => {
    const { app } = makeApp()
    const res = await patchStatus(app, 'nonexistent-worker', 'draining')
    expect(res.status).toBe(404)
  })

  it('requires worker:manage scope', async () => {
    const { app, manager } = makeApp({ scope: ['event:subscribe'] })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const res = await patchStatus(app, worker.id, 'draining')
    expect(res.status).toBe(403)
  })

  it('persists the status change when queried via GET', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    await patchStatus(app, worker.id, 'draining')

    const getRes = await app.request(`/workers/${worker.id}`)
    expect(getRes.status).toBe(200)
    const body = await getRes.json()
    expect(body.status).toBe('draining')
  })
})
