# Suspended States & Worker Protocol Design

**Date:** 2026-03-03
**Status:** Draft

## Overview

TaskCast 扩展为任务协调中枢，新增两大能力：

1. **Suspended 状态（paused/blocked）** — 任务暂停与阻塞的一等支持，含冷热存储管理
2. **WebSocket Worker 协议** — Worker 双向通信通道，支持 blocked 问答、外部事件推送、任务控制指令

---

## 1. State Machine

### 1.1 新增状态

| 状态 | 类别 | 语义 |
|------|------|------|
| `paused` | suspended | 用户主动暂停，控制权在发起方 |
| `blocked` | suspended | 等待外部输入，控制权在外部方 |

### 1.2 状态类别

每个状态归属一个类别，系统行为由类别驱动：

| 类别 | 状态 | SSE | TTL | 可 publish | completedAt |
|------|------|-----|-----|-----------|-------------|
| initial | `pending` | 等待，running 后推流 | 不计时 | 是 | 否 |
| active | `running` | replay + 实时推 | 倒计时 | 是 | 否 |
| suspended | `paused` | 保持连接，推事件 | **暂停** | 是 | 否 |
| suspended | `blocked` | 保持连接，推事件 | **继续** | 是 | 否 |
| terminal | `completed`, `failed`, `timeout`, `cancelled` | replay + 关闭 | 停止 | 否 | 是 |

### 1.3 转换规则

```
pending  → running | cancelled
running  → paused | blocked | completed | failed | timeout | cancelled
paused   → running | blocked | cancelled
blocked  → running | paused | cancelled | failed
```

```typescript
const ALLOWED_TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  pending:   ['running', 'cancelled'],
  running:   ['paused', 'blocked', 'completed', 'failed', 'timeout', 'cancelled'],
  paused:    ['running', 'blocked', 'cancelled'],
  blocked:   ['running', 'paused', 'cancelled', 'failed'],
  completed: [],
  failed:    [],
  timeout:   [],
  cancelled: [],
}
```

状态图：

```
pending → running → completed | failed | timeout | cancelled
              ↕
           paused ↔ blocked
```

关键转换说明：
- `paused ↔ blocked`：允许直接转换。`blocked → paused` 的典型场景是"停钟防超时"
- `blocked → failed`：等待的条件永远不会满足（审批被拒、外部服务下线）
- `paused → failed`：不允许。暂停是自愿的，不存在"暂停失败"

### 1.4 辅助函数

```typescript
export const SUSPENDED_STATUSES: readonly TaskStatus[] = ['paused', 'blocked'] as const

export function isSuspended(status: TaskStatus): boolean {
  return SUSPENDED_STATUSES.includes(status)
}
```

---

## 2. TTL & 唤醒定时器

### 2.1 两种独立的定时器

| 机制 | 触发后果 | 字段 |
|------|---------|------|
| **Task TTL** | → `timeout` 终态（任务死了） | `task.ttl`（现有） |
| **唤醒定时器** | → `running`（"不等了"，交还 worker 继续） | 新增 `task.resumeAt` |

两者独立并行。blocked 状态下：
- 唤醒定时器先到 → `blocked → running`，worker 自行决策（降级/重试/fail）
- Task TTL 先到 → `blocked → timeout`，任务结束

### 2.2 TTL 转换规则

核心原则：**paused = 停钟，blocked = 走钟**

| 转换 | Task TTL 操作 |
|------|--------------|
| → `paused` | clearTTL（停钟） |
| → `blocked` | 不动（继续走） |
| `paused` → `running` | 重置完整 TTL |
| `blocked` → `running` | 不动（继续走） |
| `paused` → `blocked` | 重置完整 TTL（并让它走） |
| `blocked` → `paused` | clearTTL（停钟） |

### 2.3 Transition Payload 扩展

```typescript
interface TransitionPayload {
  result?: Record<string, unknown>   // 现有
  error?: TaskError                  // 现有
  reason?: string                    // 新增：当前状态的人类可读原因
  ttl?: number                       // 新增：覆盖 task TTL（秒）
  resumeAfterMs?: number             // 新增：唤醒定时器（仅 → blocked）
}
```

`ttl` 覆盖可用于任何转换，业务方可以在 `→ blocked` 时设更短的 TTL。

### 2.4 Task 新增字段

```typescript
interface Task {
  // ... 现有字段 ...
  reason?: string       // 当前状态原因（进入 suspended 时设置，离开时清除）
  resumeAt?: number     // 唤醒时间戳（blocked 专用，ms timestamp）
}
```

---

## 3. WebSocket Worker 协议

### 3.1 连接

```
WS /workers/:workerId/ws
```

Worker 通过 WebSocket 建立持久双向连接。认证方式复用现有 JWT/custom auth，通过 query parameter 或首条消息传递 token。

### 3.2 消息格式

所有消息使用 JSON，包含 `type` 字段区分消息类型。

#### Worker → TaskCast

```typescript
// 发布事件
{ type: "publish", taskId: string, event: PublishEventInput }

// 状态转换
{ type: "transition", taskId: string, status: TaskStatus, payload?: TransitionPayload }

// 发起 blocked 请求（transition to blocked 的语法糖）
{ type: "block", taskId: string, reason: string, request: BlockedRequest, resumeAfterMs?: number }
```

#### TaskCast → Worker

```typescript
// 状态指令（用户 pause/cancel 等）
{ type: "status_changed", taskId: string, status: TaskStatus, reason?: string }

// Blocked 解决结果
{ type: "blocked_resolved", taskId: string, resolution: unknown }

// 唤醒通知（resume timer 到期）
{ type: "resume_timeout", taskId: string }

// 外部事件（通过 REST signal 推送的）
{ type: "signal", taskId: string, signalType: string, data: unknown }

// 任务变冷通知
{ type: "task_cold", taskId: string }
```

### 3.3 BlockedRequest

Worker 发起 blocked 时可以附带结构化请求，描述需要外部提供什么：

```typescript
interface BlockedRequest {
  type: string                        // 请求类型，如 "approval", "input", "confirmation"
  data: unknown                       // 请求详情
}
```

存储在 Task 上：

```typescript
interface Task {
  // ...
  blockedRequest?: BlockedRequest     // 当前 blocked 请求（blocked 时设置，resolve 时清除）
}
```

### 3.4 任务与 Worker 绑定

```typescript
interface Task {
  // ...
  workerId?: string       // 绑定的 WebSocket worker
  workerOnly?: boolean    // 是否要求 WebSocket worker
}
```

`workerOnly: true` 的任务只允许 WebSocket 连接的 worker 接取和执行。

---

## 4. Blocked Request/Resolve 流程

### 4.1 完整生命周期

```
Worker: block(taskId, { reason, request, resumeAfterMs })
   │
   ▼
TaskCast:
  1. transition → blocked
  2. 存储 blockedRequest 到 Task
  3. 设置 resumeAt = now + resumeAfterMs（如果提供）
  4. 发送 taskcast:status SSE 事件（通知 Client）
  5. 发送 taskcast:blocked SSE 事件（含 request 详情，给 UI 渲染）
   │
   ├─ 路径 A：外部通过 REST 回复
   │    POST /tasks/:id/resolve { data: { answer: "approved" } }
   │    → task.status = running, 清除 blockedRequest
   │    → WebSocket 推 blocked_resolved 给 Worker（含 resolution data）
   │    → SSE 推 taskcast:status（通知 Client 任务恢复）
   │
   ├─ 路径 B：唤醒定时器到期（"不等了"）
   │    → task.status = running, 清除 blockedRequest
   │    → WebSocket 推 resume_timeout 给 Worker
   │    → Worker 自行决策（降级/重试/fail）
   │
   ├─ 路径 C：用户暂停
   │    → task.status = paused
   │    → WebSocket 推 status_changed 给 Worker
   │    → 唤醒定时器取消，TTL 停钟
   │
   └─ 路径 D：用户取消
        → task.status = cancelled
        → WebSocket 推 status_changed 给 Worker
        → Worker 停止执行
```

### 4.2 REST 端点

```
POST /tasks/:id/resolve
  Body: { data: unknown }
  权限: task:resolve
  行为: blocked → running，resolution 推给 Worker

GET /tasks/:id/request
  权限: task:resolve（读取也需要，因为可能包含敏感信息）
  返回: 当前 blockedRequest，如果 task 不是 blocked 返回 404

POST /tasks/:id/signal
  Body: { type: string, data: unknown }
  权限: task:signal
  行为: 通过 WebSocket 推送给 Worker，不改变状态
```

---

## 5. Hot/Cold Task Mechanism

### 5.1 定义

| 类型 | 存储 | 更新方式 | SSE |
|------|------|---------|-----|
| **热任务** | ShortTermStore + LongTermStore | 实时广播 | 保持连接 |
| **冷任务** | 仅 LongTermStore | 轮询 | 关闭连接 |

### 5.2 降级规则

| 状态 | 降级阈值（可配置） |
|------|------------------|
| `paused` | 5 分钟 |
| `blocked`（无 resumeAfterMs） | 30 分钟 |
| `blocked`（有 resumeAfterMs） | resumeAfterMs 后若仍 blocked，再等 5 分钟 |

### 5.3 降级流程

1. 发送 `taskcast:cold` SSE 事件
2. 通过 WebSocket 发送 `task_cold` 给 Worker
3. 关闭该 task 的 SSE 连接
4. 从 ShortTermStore 中移除
5. 数据仍在 LongTermStore

### 5.4 升级流程（冷 → 热）

当冷任务收到 transition 请求（如 `→ running`）或唤醒定时器触发：

1. 从 LongTermStore 加载 Task 到 ShortTermStore
2. 恢复广播能力
3. 如果 Worker 仍然连接，通过 WebSocket 通知
4. Client 可重新订阅 SSE

### 5.5 后台调度器

```typescript
interface TaskSchedulerOptions {
  pausedColdAfterMs: number      // 默认 5 * 60 * 1000
  blockedColdAfterMs: number     // 默认 30 * 60 * 1000
  checkIntervalMs: number        // 默认 60 * 1000
}
```

调度器统一负责：
1. **唤醒定时器**：扫描 `status === 'blocked' && resumeAt <= now`，自动 transition to running
2. **冷热降级**：扫描 suspended 任务超过阈值，执行降级

调度器查询 ShortTermStore（热任务）和 LongTermStore（唤醒定时器可能在冷任务上），确保冷任务也能被唤醒。

---

## 6. Permission Scopes

### 6.1 新增权限

```typescript
export type PermissionScope =
  | 'task:create'
  | 'task:manage'
  | 'task:resolve'        // 新增：回复 blocked 请求
  | 'task:signal'         // 新增：给 worker 推送外部事件
  | 'event:publish'
  | 'event:subscribe'
  | 'event:history'
  | 'webhook:create'
  | 'worker:connect'      // 新增：WebSocket worker 连接
  | '*'
```

### 6.2 端点权限映射

| 端点 | 所需权限 |
|------|---------|
| `POST /tasks/:id/resolve` | `task:resolve` |
| `GET /tasks/:id/request` | `task:resolve` |
| `POST /tasks/:id/signal` | `task:signal` |
| `WS /workers/:workerId/ws` | `worker:connect` |
| `PATCH /tasks/:id/status`（现有） | `task:manage` |

---

## 7. SSE Behavior

suspended 状态下 SSE 行为和 running 一致：保持连接，推送事件。

新增 SSE 事件类型：

| 事件 | 触发时机 | data |
|------|---------|------|
| `taskcast:status` | 状态变更（现有，含 paused/blocked） | `{ status, reason? }` |
| `taskcast:blocked` | 进入 blocked 并附带 request | `{ reason, request }` |
| `taskcast:resolved` | blocked 被 resolve | `{ resolution }` |
| `taskcast:cold` | 任务降级为冷任务 | `{}` |

---

## 8. Storage Changes

### 8.1 ShortTermStore 接口新增

```typescript
interface ShortTermStore {
  // ... 现有方法 ...
  clearTTL?(taskId: string): Promise<void>                     // 清除 TTL（paused 停钟）
  listByStatus?(status: TaskStatus[]): Promise<Task[]>         // 调度器用
}
```

两个方法均为可选（`?`），向后兼容。

### 8.2 各适配器实现

| 适配器 | clearTTL | listByStatus |
|--------|----------|-------------|
| Memory | 清除 timer | filter tasks |
| Redis | `PERSIST` 命令 | `SCAN` + filter |
| SQLite | 清除 TTL 记录 | SQL query |

---

## 9. Component Impact Summary

| 组件 | 改动范围 |
|------|---------|
| `core/types.ts` | TaskStatus 加 paused/blocked，Task 加 reason/resumeAt/blockedRequest/workerId/workerOnly |
| `core/state-machine.ts` | 转换表、isSuspended()、SUSPENDED_STATUSES |
| `core/engine.ts` | transition TTL 逻辑、reason 处理、唤醒定时器设置 |
| 新增 `core/scheduler.ts` | 后台调度器（唤醒 + 冷热降级） |
| `server/routes/tasks.ts` | Zod schema 扩展、新增 resolve/request/signal 端点 |
| `server/routes/sse.ts` | 无改动（suspended 走非 terminal 路径） |
| 新增 `server/routes/workers.ts` | WebSocket worker 端点 |
| `server/auth.ts` | 新增权限 scope |
| ShortTermStore 接口 | 新增 clearTTL、listByStatus |
| 所有存储适配器 | 实现新接口方法 |
| `server-sdk` | 新增 resolve/signal/block 方法 |
| `client` / `react` | 处理新事件类型（taskcast:blocked/resolved/cold） |
| **Rust 全部对应组件** | 同步更新 |

---

## 10. Backward Compatibility

**完全向后兼容。**

- 不使用 paused/blocked 的用户，所有行为不变
- `reason`、`resumeAt`、`blockedRequest` 等字段全部可选
- `clearTTL`、`listByStatus` 为可选方法（`?.` 调用）
- 现有 SSE 事件格式不变，新事件类型是增量
- 现有权限 scope 不变，新 scope 是增量
- REST worker（server-sdk）仍然工作，WebSocket 是新增选项
