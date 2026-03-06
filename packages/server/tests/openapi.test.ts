import { describe, it, expect, beforeAll } from 'vitest'
import { createTaskcastApp } from '../src/index.js'
import type { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'

describe('OpenAPI', () => {
  let app: Hono

  beforeAll(async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    ;({ app } = createTaskcastApp({ engine, auth: { mode: 'none' } }))
  })

  it('GET /openapi.json returns valid OpenAPI spec', async () => {
    const res = await app.request('/openapi.json')
    expect(res.status).toBe(200)
    const spec = await res.json()
    expect(spec.openapi).toBe('3.1.0')
    expect(spec.info.title).toBe('Taskcast API')
    expect(spec.paths).toBeDefined()
    // Check key paths exist
    expect(spec.paths['/tasks']).toBeDefined()
    expect(spec.paths['/tasks/{taskId}']).toBeDefined()
  })

  it('GET /openapi.json includes Bearer security scheme', async () => {
    const res = await app.request('/openapi.json')
    const spec = await res.json()
    expect(spec.components?.securitySchemes?.Bearer).toBeDefined()
    expect(spec.components.securitySchemes.Bearer.type).toBe('http')
    expect(spec.components.securitySchemes.Bearer.scheme).toBe('bearer')
  })

  it('GET /openapi.json includes all route tags', async () => {
    const res = await app.request('/openapi.json')
    const spec = await res.json()
    // Collect all tags from paths
    const tags = new Set<string>()
    for (const path of Object.values(spec.paths) as any[]) {
      for (const method of Object.values(path) as any[]) {
        if (method.tags) method.tags.forEach((t: string) => tags.add(t))
      }
    }
    expect(tags.has('Tasks')).toBe(true)
    expect(tags.has('Events')).toBe(true)
  })

  it('GET /docs returns HTML', async () => {
    const res = await app.request('/docs')
    expect(res.status).toBe(200)
    const ct = res.headers.get('content-type')
    expect(ct).toContain('text/html')
  })
})
