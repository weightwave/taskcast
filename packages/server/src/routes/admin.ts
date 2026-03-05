import { Hono } from 'hono'
import { SignJWT } from 'jose'
import type { TaskcastConfig, PermissionScope } from '@taskcast/core'
import type { AuthConfig } from '../auth.js'

export interface AdminRouteOptions {
  config: TaskcastConfig
  auth?: AuthConfig | undefined
}

/**
 * Creates the admin router with POST /token endpoint.
 *
 * This route is intentionally NOT behind the normal auth middleware.
 * It authenticates via admin token and issues JWTs (when auth mode is jwt).
 */
export function createAdminRouter(opts: AdminRouteOptions): Hono {
  const router = new Hono()

  router.post('/token', async (c) => {
    // 1. If adminApi is not enabled, this endpoint does not exist
    if (!opts.config.adminApi) {
      return c.json({ error: 'Not found' }, 404)
    }

    // 2. Parse and validate request body
    let body: Record<string, unknown>
    try {
      body = await c.req.json()
    } catch {
      return c.json({ error: 'Invalid request body' }, 400)
    }

    const adminToken = body.adminToken
    if (typeof adminToken !== 'string' || adminToken === '') {
      return c.json({ error: 'Invalid admin token' }, 401)
    }

    // 3. Validate admin token — ALWAYS, regardless of server auth mode
    if (adminToken !== opts.config.adminToken) {
      return c.json({ error: 'Invalid admin token' }, 401)
    }

    // 4. Parse optional fields
    const scopes: PermissionScope[] = Array.isArray(body.scopes) ? body.scopes : ['*']
    const expiresIn: number = typeof body.expiresIn === 'number' && body.expiresIn > 0
      ? body.expiresIn
      : 86400 // default: 24h

    const expiresAt = Math.floor(Date.now() / 1000) + expiresIn

    // 5. If auth mode is JWT, sign a real token
    if (opts.auth?.mode === 'jwt' && opts.auth.jwt?.secret) {
      const secret = new TextEncoder().encode(opts.auth.jwt.secret)
      const algorithm = opts.auth.jwt.algorithm ?? 'HS256'

      const jwt = await new SignJWT({
        sub: 'admin',
        scope: scopes,
        taskIds: '*',
      })
        .setProtectedHeader({ alg: algorithm })
        .setExpirationTime(expiresAt)
        .setIssuedAt()
        .sign(secret)

      return c.json({ token: jwt, expiresAt })
    }

    // 6. Non-JWT mode (e.g., "none"): still validated admin token above (double insurance!)
    //    Return placeholder since no JWT system is configured
    return c.json({ token: '', expiresAt })
  })

  return router
}