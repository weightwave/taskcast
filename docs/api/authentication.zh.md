# 认证与权限

Taskcast 提供灵活的认证系统，支持从完全开放到细粒度权限控制。

## 认证模式

### none（默认）

不进行任何认证，所有请求都被允许。适用于开发环境或内网部署。

```yaml
auth:
  mode: none
```

### jwt

使用 JWT（JSON Web Token）进行认证。支持多种签名算法。

```yaml
auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: your-secret-key
```

### custom（仅 TS/JS 配置）

使用自定义中间件函数进行认证。适用于集成已有认证系统（如 OAuth、Session 等）。

```typescript
// taskcast.config.ts
export default {
  auth: {
    mode: 'custom',
    middleware: async (req: Request) => {
      const session = await validateSession(req)
      if (!session) return null // 未认证
      return {
        sub: session.userId,
        taskIds: '*',
        scope: ['event:subscribe', 'event:history'],
      }
    },
  },
}
```

## JWT 配置

### 支持的算法

| 算法 | 类型 | 配置方式 |
|------|------|----------|
| `HS256` | HMAC | `secret` |
| `RS256` | RSA | `publicKey` 或 `publicKeyFile` |
| `ES256` | ECDSA P-256 | `publicKey` 或 `publicKeyFile` |
| `ES384` | ECDSA P-384 | `publicKey` 或 `publicKeyFile` |
| `ES512` | ECDSA P-521 | `publicKey` 或 `publicKeyFile` |

### 配置示例

**HMAC（对称密钥）：**

```yaml
auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: ${JWT_SECRET}
```

**RSA（非对称密钥）：**

```yaml
auth:
  mode: jwt
  jwt:
    algorithm: RS256
    publicKeyFile: /run/secrets/jwt.pub
    issuer: my-auth-service    # 可选，验证签发者
    audience: taskcast         # 可选，验证受众
```

## JWT Payload

Taskcast 期望 JWT payload 中包含以下字段：

```typescript
interface TaskcastJWTPayload {
  sub?: string                 // 主体标识（用户 ID 等）
  taskIds: string[] | '*'      // 可访问的任务 ID 列表，'*' 表示所有
  scope: PermissionScope[]     // 权限范围列表
  exp?: number                 // 过期时间（Unix timestamp）
}
```

### 示例

**全权限 token（后端服务间通信）：**

```json
{
  "sub": "backend-service",
  "taskIds": "*",
  "scope": ["*"],
  "exp": 1700003600
}
```

**受限 token（前端用户）：**

```json
{
  "sub": "user-123",
  "taskIds": ["task-001", "task-002"],
  "scope": ["event:subscribe", "event:history"],
  "exp": 1700003600
}
```

**单任务 token（分享链接）：**

```json
{
  "sub": "anonymous",
  "taskIds": ["task-001"],
  "scope": ["event:subscribe"],
  "exp": 1700003600
}
```

## 权限范围（Scope）

| Scope | 说明 | 涉及端点 |
|-------|------|----------|
| `task:create` | 创建任务 | `POST /tasks` |
| `task:manage` | 更改任务状态、删除任务 | `PATCH /tasks/:id/status`, `DELETE /tasks/:id` |
| `event:publish` | 向任务发布事件 | `POST /tasks/:id/events` |
| `event:subscribe` | 订阅任务 SSE 流 | `GET /tasks/:id/events` |
| `event:history` | 查询事件历史 | `GET /tasks/:id/events/history` |
| `webhook:create` | 在创建任务时配置 webhook | `POST /tasks`（webhooks 字段） |
| `*` | 完全访问权限（包含以上所有） | 所有端点 |

### 权限检查逻辑

每个请求需要同时满足：

1. **scope 匹配** — token 的 `scope` 包含该端点所需的权限
2. **taskId 匹配** — token 的 `taskIds` 包含请求的任务 ID（或为 `'*'`）

## 任务级权限（authConfig）

除了全局认证配置，每个任务还可以定义自己的额外权限规则：

```typescript
const task = await engine.createTask({
  type: 'private.chat',
  authConfig: {
    rules: [
      {
        match: { scope: ['event:subscribe'] },
        require: {
          sub: ['user-123', 'user-456'], // 只有这些用户可以订阅
        },
      },
      {
        match: { scope: ['event:publish'] },
        require: {
          claims: { role: 'admin' }, // 只有 admin 角色可以发布事件
        },
      },
    ],
  },
})
```

任务级权限是在全局权限检查之后的**额外**约束，不会放宽全局权限。

## 请求认证方式

所有需要认证的请求通过 `Authorization` 头传递 token：

```
Authorization: Bearer <jwt-token>
```

SSE 连接也使用同样的方式：

```javascript
const eventSource = new EventSource('/tasks/xxx/events', {
  headers: {
    Authorization: `Bearer ${token}`,
  },
})
```

在使用 `@taskcast/client` 或 `@taskcast/react` 时，通过 `token` 选项传递：

```typescript
const client = new TaskcastClient({
  baseUrl: 'http://localhost:3721',
  token: jwtToken,
})
```