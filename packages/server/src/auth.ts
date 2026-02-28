import { createMiddleware } from 'hono/factory'
import { jwtVerify, importSPKI, type KeyLike } from 'jose'
import type { PermissionScope } from '@taskcast/core'

export interface JWTConfig {
  algorithm: 'HS256' | 'RS256' | 'ES256' | 'ES384' | 'ES512'
  secret?: string
  publicKey?: string
  publicKeyFile?: string
  issuer?: string
  audience?: string
}

export type AuthMode = 'none' | 'jwt' | 'custom'

export interface AuthConfig {
  mode: AuthMode
  jwt?: JWTConfig
  middleware?: (req: Request) => Promise<AuthContext | null>
}

export interface AuthContext {
  sub?: string
  taskIds: string[] | '*'
  scope: PermissionScope[]
}

declare module 'hono' {
  interface ContextVariableMap {
    auth: AuthContext
  }
}

const OPEN_AUTH: AuthContext = { taskIds: '*', scope: ['*'] }

export function createAuthMiddleware(config: AuthConfig) {
  return createMiddleware(async (c, next) => {
    if (config.mode === 'none') {
      c.set('auth', OPEN_AUTH)
      return next()
    }

    if (config.mode === 'custom' && config.middleware) {
      const ctx = await config.middleware(c.req.raw)
      if (!ctx) return c.json({ error: 'Unauthorized' }, 401)
      c.set('auth', ctx)
      return next()
    }

    if (config.mode === 'jwt' && config.jwt) {
      const authHeader = c.req.header('Authorization')
      if (!authHeader?.startsWith('Bearer ')) {
        return c.json({ error: 'Missing Bearer token' }, 401)
      }
      const token = authHeader.slice(7)
      try {
        const key = await resolveKey(config.jwt)
        const jwtOptions: Parameters<typeof jwtVerify>[2] = {}
        if (config.jwt.issuer !== undefined) jwtOptions.issuer = config.jwt.issuer
        if (config.jwt.audience !== undefined) jwtOptions.audience = config.jwt.audience
        const { payload } = await jwtVerify(token, key, jwtOptions)
        const ctx: AuthContext = {
          taskIds: (payload['taskIds'] as string[] | '*') ?? '*',
          scope: (payload['scope'] as PermissionScope[]) ?? [],
        }
        if (payload.sub !== undefined) ctx.sub = payload.sub
        c.set('auth', ctx)
        return next()
      } catch {
        return c.json({ error: 'Invalid or expired token' }, 401)
      }
    }

    return c.json({ error: 'Unauthorized' }, 401)
  })
}

async function resolveKey(cfg: JWTConfig): Promise<KeyLike | Uint8Array> {
  if (cfg.secret) {
    return new TextEncoder().encode(cfg.secret)
  }
  if (cfg.publicKey) {
    return importSPKI(cfg.publicKey, cfg.algorithm)
  }
  if (cfg.publicKeyFile) {
    const { readFileSync } = await import('fs')
    const pem = readFileSync(cfg.publicKeyFile, 'utf8')
    return importSPKI(pem, cfg.algorithm)
  }
  throw new Error('JWT config requires secret or publicKey or publicKeyFile')
}

export function checkScope(
  auth: AuthContext,
  required: PermissionScope,
  taskId?: string,
): boolean {
  if (taskId && auth.taskIds !== '*') {
    if (!auth.taskIds.includes(taskId)) return false
  }
  return auth.scope.includes('*') || auth.scope.includes(required)
}
