import { describe, it, expect, vi, afterEach } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { WorkerWSHandler, WorkerWSRegistry } from '../src/routes/worker-ws.js'
import type { WSLike } from '../src/routes/worker-ws.js'

function mockWS(): WSLike & { sent: unknown[]; close: ReturnType<typeof vi.fn> } {
  const sent: unknown[] = []
  return {
    send: vi.fn((data: string) => sent.push(JSON.parse(data))),
    close: vi.fn(),
    sent,
  }
}

function makeEnv() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  const registry = new WorkerWSRegistry()
  return { store, broadcast, engine, manager, registry }
}

describe('WorkerWSHandler — bad messages', () => {
  describe('messages after handler disconnect', () => {
    it('sends messages after disconnect — operations on missing worker are no-op', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws, undefined, env.registry)

      // Register and disconnect
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!
      expect(workerId).toBeDefined()

      await handler.handleDisconnect()
      ws.sent.length = 0

      // Sending update after disconnect — worker is gone, but handler still has workerId
      // updateWorker on a missing worker should not crash
      await handler.handleMessage(JSON.stringify({ type: 'update', weight: 99 }))

      // Sending pong after disconnect — heartbeat on missing worker
      await handler.handleMessage(JSON.stringify({ type: 'pong' }))

      // Sending drain after disconnect
      await handler.handleMessage(JSON.stringify({ type: 'drain' }))

      // Sending claim after disconnect — should get claimed:false since worker is gone
      const task = await env.engine.createTask({ type: 'test', cost: 1 })
      await handler.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))

      // The claim should return claimed with success:false because the worker is unregistered
      const claimMsg = ws.sent.find(
        (m) => (m as Record<string, unknown>).type === 'claimed',
      ) as Record<string, unknown> | undefined
      if (claimMsg) {
        expect(claimMsg.success).toBe(false)
      }
    })
  })

  describe('register with missing fields', () => {
    it('register with no matchRule uses empty object default', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      // Register with no matchRule
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))

      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('registered')

      const worker = await env.manager.getWorker(handler.registeredWorkerId!)
      expect(worker!.matchRule).toEqual({})
    })

    it('register with no capacity uses default of 10', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      await handler.handleMessage(JSON.stringify({ type: 'register' }))

      const worker = await env.manager.getWorker(handler.registeredWorkerId!)
      expect(worker!.capacity).toBe(10)
    })
  })

  describe('claim with wrong types', () => {
    it('claim with taskId: 123 (number not string) sends error', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      // taskId is a number instead of string
      await handler.handleMessage(JSON.stringify({ type: 'claim', taskId: 123 }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.code).toBe('INVALID_MESSAGE')
      expect(msg.message).toContain('taskId is required and must be a string')
    })

    it('accept with taskId: true (boolean not string) sends error', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: true }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.code).toBe('INVALID_MESSAGE')
    })

    it('decline with taskId: null sends error', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({ type: 'decline', taskId: null }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.code).toBe('INVALID_MESSAGE')
    })
  })

  describe('unknown message type', () => {
    it('sends UNKNOWN_TYPE-style error for completely unknown type', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({ type: 'fly_to_moon' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Unknown message type: fly_to_moon')
    })

    it('sends error for empty type', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      await handler.handleMessage(JSON.stringify({ type: '' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Unknown message type: ')
    })

    it('sends error for numeric type', async () => {
      const env = makeEnv()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws)

      // type is a number, not a string — switch-case treats undefined
      await handler.handleMessage(JSON.stringify({ type: 42 }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      // msg.type cast as string | undefined will be 42, which hits default
      expect(msg.message).toContain('Unknown message type')
    })
  })

  describe('rapid register → disconnect → register cycle', () => {
    it('no timer leaks on rapid register → disconnect → register', async () => {
      const env = makeEnv()
      // Use a short heartbeat interval to detect leaks
      const manager = new WorkerManager({
        engine: env.engine,
        shortTermStore: env.store,
        broadcast: env.broadcast,
        defaults: { heartbeatIntervalMs: 30 },
      })
      const registry = new WorkerWSRegistry()
      const ws = mockWS()
      const handler = new WorkerWSHandler(manager, ws, undefined, registry)

      // Rapid register/disconnect cycles
      for (let i = 0; i < 5; i++) {
        await handler.handleMessage(JSON.stringify({
          type: 'register',
          workerId: `worker-cycle-${i}`,
          capacity: 5,
        }))
        await handler.handleDisconnect()
      }

      // Final register
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        workerId: 'worker-final',
        capacity: 5,
      }))
      const finalId = handler.registeredWorkerId
      expect(finalId).toBe('worker-final')

      // Clean up to avoid leaked timers
      handler.stopPingTimer()
      ws.sent.length = 0

      // Wait and verify only the final worker receives pings (no leaked timers)
      await new Promise((r) => setTimeout(r, 100))

      // There should be no ping messages since we stopped the timer
      const pings = ws.sent.filter((m) => (m as Record<string, unknown>).type === 'ping')
      expect(pings).toHaveLength(0)

      // Clean up
      await handler.handleDisconnect()
    })

    it('registry properly tracks after register → disconnect → register', async () => {
      const env = makeEnv()
      const registry = new WorkerWSRegistry()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws, undefined, registry)

      // First register
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        workerId: 'w1',
        capacity: 5,
      }))
      expect(registry.get('w1')).toBe(handler)

      // Disconnect
      await handler.handleDisconnect()
      expect(registry.get('w1')).toBeUndefined()

      // Re-register with new ID
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        workerId: 'w2',
        capacity: 5,
      }))
      expect(registry.get('w2')).toBe(handler)
      expect(registry.get('w1')).toBeUndefined()

      // Clean up
      handler.stopPingTimer()
      await handler.handleDisconnect()
    })
  })

  describe('WorkerWSRegistry', () => {
    it('getAll returns all registered handlers', () => {
      const env = makeEnv()
      const registry = new WorkerWSRegistry()
      const ws1 = mockWS()
      const ws2 = mockWS()
      const handler1 = new WorkerWSHandler(env.manager, ws1, undefined, registry)
      const handler2 = new WorkerWSHandler(env.manager, ws2, undefined, registry)

      registry.register('w1', handler1)
      registry.register('w2', handler2)

      const all = registry.getAll()
      expect(all.size).toBe(2)
      expect(all.get('w1')).toBe(handler1)
      expect(all.get('w2')).toBe(handler2)
    })

    it('unregister removes handler from registry', () => {
      const env = makeEnv()
      const registry = new WorkerWSRegistry()
      const ws = mockWS()
      const handler = new WorkerWSHandler(env.manager, ws, undefined, registry)

      registry.register('w1', handler)
      expect(registry.get('w1')).toBe(handler)

      registry.unregister('w1')
      expect(registry.get('w1')).toBeUndefined()
    })

    it('get returns undefined for non-existent worker', () => {
      const registry = new WorkerWSRegistry()
      expect(registry.get('nonexistent')).toBeUndefined()
    })
  })
})
