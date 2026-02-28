# Authentication & Authorization

Taskcast provides a flexible authentication system ranging from fully open access to fine-grained permission control.

## Authentication Modes

### none (default)

No authentication is performed — all requests are allowed. Suitable for development environments or internal network deployments.

```yaml
auth:
  mode: none
```

### jwt

Authenticate using JWT (JSON Web Token). Supports multiple signing algorithms.

```yaml
auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: your-secret-key
```

### custom (TS/JS config only)

Authenticate using a custom middleware function. Useful for integrating with existing authentication systems such as OAuth or session-based auth.

```typescript
// taskcast.config.ts
export default {
  auth: {
    mode: 'custom',
    middleware: async (req: Request) => {
      const session = await validateSession(req)
      if (!session) return null // unauthenticated
      return {
        sub: session.userId,
        taskIds: '*',
        scope: ['event:subscribe', 'event:history'],
      }
    },
  },
}
```

## JWT Configuration

### Supported Algorithms

| Algorithm | Type | Configuration |
|-----------|------|---------------|
| `HS256` | HMAC | `secret` |
| `RS256` | RSA | `publicKey` or `publicKeyFile` |
| `ES256` | ECDSA P-256 | `publicKey` or `publicKeyFile` |
| `ES384` | ECDSA P-384 | `publicKey` or `publicKeyFile` |
| `ES512` | ECDSA P-521 | `publicKey` or `publicKeyFile` |

### Configuration Examples

**HMAC (symmetric key):**

```yaml
auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: ${JWT_SECRET}
```

**RSA (asymmetric key):**

```yaml
auth:
  mode: jwt
  jwt:
    algorithm: RS256
    publicKeyFile: /run/secrets/jwt.pub
    issuer: my-auth-service    # optional, validates the token issuer
    audience: taskcast         # optional, validates the token audience
```

## JWT Payload

Taskcast expects the following fields in the JWT payload:

```typescript
interface TaskcastJWTPayload {
  sub?: string                 // subject identifier (user ID, etc.)
  taskIds: string[] | '*'      // list of accessible task IDs, or '*' for all
  scope: PermissionScope[]     // list of permission scopes
  exp?: number                 // expiration time (Unix timestamp)
}
```

### Examples

**Full-access token (backend service-to-service communication):**

```json
{
  "sub": "backend-service",
  "taskIds": "*",
  "scope": ["*"],
  "exp": 1700003600
}
```

**Restricted token (frontend user):**

```json
{
  "sub": "user-123",
  "taskIds": ["task-001", "task-002"],
  "scope": ["event:subscribe", "event:history"],
  "exp": 1700003600
}
```

**Single-task token (shared link):**

```json
{
  "sub": "anonymous",
  "taskIds": ["task-001"],
  "scope": ["event:subscribe"],
  "exp": 1700003600
}
```

## Permission Scopes

| Scope | Description | Endpoints |
|-------|-------------|-----------|
| `task:create` | Create a task | `POST /tasks` |
| `task:manage` | Change task status, delete a task | `PATCH /tasks/:id/status`, `DELETE /tasks/:id` |
| `event:publish` | Publish events to a task | `POST /tasks/:id/events` |
| `event:subscribe` | Subscribe to a task's SSE stream | `GET /tasks/:id/events` |
| `event:history` | Query event history | `GET /tasks/:id/events/history` |
| `webhook:create` | Configure webhooks when creating a task | `POST /tasks` (webhooks field) |
| `*` | Full access (includes all of the above) | All endpoints |

### Authorization Check Logic

Each request must satisfy both of the following conditions:

1. **Scope match** — the token's `scope` includes the permission required by the endpoint
2. **Task ID match** — the token's `taskIds` includes the requested task ID (or is `'*'`)

## Task-Level Permissions (authConfig)

In addition to the global authentication configuration, each task can define its own supplementary permission rules:

```typescript
const task = await engine.createTask({
  type: 'private.chat',
  authConfig: {
    rules: [
      {
        match: { scope: ['event:subscribe'] },
        require: {
          sub: ['user-123', 'user-456'], // only these users may subscribe
        },
      },
      {
        match: { scope: ['event:publish'] },
        require: {
          claims: { role: 'admin' }, // only users with the admin role may publish events
        },
      },
    ],
  },
})
```

Task-level permissions are **additional** constraints applied after the global permission check — they cannot relax global permissions.

## Passing Authentication Credentials

All requests requiring authentication pass the token via the `Authorization` header:

```
Authorization: Bearer <jwt-token>
```

SSE connections use the same mechanism:

```javascript
const eventSource = new EventSource('/tasks/xxx/events', {
  headers: {
    Authorization: `Bearer ${token}`,
  },
})
```

When using `@taskcast/client` or `@taskcast/react`, pass the token via the `token` option:

```typescript
const client = new TaskcastClient({
  baseUrl: 'http://localhost:3721',
  token: jwtToken,
})
```