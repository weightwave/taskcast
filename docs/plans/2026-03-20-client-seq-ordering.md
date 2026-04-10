# Client-Side Sequence Ordering

**Date:** 2026-03-20
**Status:** Approved

## Problem

当客户端通过多个 HTTP 请求顺序发布事件时，网络抖动或负载均衡可能导致事件到达服务端的顺序与发送顺序不一致。服务端按到达顺序分配 `index`，因此 `getHistory()` 返回的事件顺序与客户端的逻辑顺序不符。

**具体案例（claw-hive 中观察到）：**

客户端逻辑顺序：`message_end → llm_call_end → turn_end → agent_end`

但 `getHistory()` 返回：`message_end → turn_end → agent_end → llm_call_end`

实时 SSE（使用进程内 broadcast）不受影响，仅在读取历史记录时出现乱序。

## Design

### 核心思路

在 `PublishEventInput` 中新增 `clientId` + `clientSeq` 字段。服务端维护 per-`(taskId, clientId)` 的 `expectedSeq` 状态，对乱序到达的事件进行 hold（等待缺失的 seq 补齐后按序释放）或 fast-fail（立即拒绝）。

不是事后排序，而是**写入时保序** —— 保证 `index` 分配严格按 `clientSeq` 顺序。

### 新增字段

```typescript
// PublishEventInput 新增
interface PublishEventInput {
  // ...existing
  clientId?: string           // publisher 标识（ULID）
  clientSeq?: number          // per (taskId, clientId) 连续递增，任意起始值
  seqMode?: 'hold' | 'fast-fail'  // 默认 'hold'
}

// TaskEvent 持久化新增
interface TaskEvent {
  // ...existing
  clientId?: string
  clientSeq?: number
}
```

约束：
- `clientId` 和 `clientSeq` 必须同时提供或同时缺失
- 都不提供 → 完全走现有逻辑，零影响
- `seqMode` 仅在 `clientId` + `clientSeq` 存在时生效
- `clientSeq` 任意起始值，之后必须连续递增（+1）

### Seq 状态模型

Per `(taskId, clientId)` 的运行时状态：

```
expectedSeq: number | null    # null = 未初始化，首个事件到达时设为 seq + 1
slots: Set<number>            # 已注册等待的 seq 集合
```

内存 adapter：`Map<string, { expected, slots }>`
Redis adapter：两个 key（均带 TTL，活动刷新 + 终态清理）：
- `seq:{taskId}:{clientId}:expected` → number
- `seq:{taskId}:{clientId}:slots` → Set

### 请求判定逻辑（Lua 原子操作）

```
process_seq(taskId, clientId, N):

  expected = GET expected  // null 表示未初始化

  if expected == null:
    SET expected = N + 1
    return ACCEPT

  if N < expected:
    return REJECT_STALE

  if N == expected:
    if slots.contains(N):
      return REJECT_DUPLICATE  // 已注册过（早到的事件占了这个位置）
    SET expected = N + 1
    if slots.contains(expected):
      return ACCEPT_AND_TRIGGER(expected)
    return ACCEPT

  // N > expected
  if slots.contains(N):
    return REJECT_DUPLICATE
  slots.add(N)
  return WAIT
```

### 链式触发

关键原则：**每个事件 `_emit` 完成后，由该事件触发下一个**，而不是在一次 Lua 调用中级联触发所有后续事件。这保证了 `index` 分配严格按 `clientSeq` 顺序。

```
seq=5 到达（matches expected）
  → Lua: accept, expected=6, 发现 slot[6] → 返回 {accept, triggerNext: 6}
  → _emit(5) 完成
  → 通知 slot[6]

seq=6 收到通知
  → _emit(6) 完成
  → Lua: advance_after_emit(6) → expected=7, 发现 slot[7] → 返回 {triggerNext: 7}
  → 通知 slot[7]

seq=7 收到通知
  → _emit(7) 完成
  → Lua: advance_after_emit(7) → expected=8, 无 slot → 链结束
```

`advance_after_emit` 逻辑：

```
advance_after_emit(taskId, clientId, completedSeq):
  expected = GET expected
  if expected == completedSeq:
    SET expected = completedSeq + 1
  next = completedSeq + 1
  if slots.contains(next):
    slots.remove(next)
    return TRIGGER(next)
  return DONE
```

### 超时与取消

```
cancel_slot(taskId, clientId, N):
  if slots.contains(N):
    slots.remove(N)
    return CANCELLED         // 超时，返回 408
  return ALREADY_TRIGGERED   // 竞态：超时瞬间被触发，按成功走
```

超时流程：
1. `cancelSlot` 原子删除注册
2. 返回 `ALREADY_TRIGGERED` → 按成功处理（去检查信号）
3. 返回 `CANCELLED` → 返回 408 给客户端

### 通知机制

复用 BroadcastProvider，频道名 `seq-trigger:{taskId}:{clientId}:{seq}`。

- 单实例 / memory adapter：`EventEmitter` 或 `Map<string, Promise resolver>`
- 多实例 / Redis：RedisBroadcastProvider pub/sub

### Engine 编排

```typescript
async publishEvent(taskId, input) {
  if (!input.clientId) return this._emit(taskId, input)  // 无 seq 走原逻辑

  const result = await this.seqStore.processSeq(taskId, input.clientId, input.clientSeq)

  switch (result.action) {
    case 'accept':
      const event = await this._emit(taskId, input)
      if (result.triggerNext) this.notifySlot(taskId, input.clientId, result.triggerNext)
      return event

    case 'wait':
      if (input.seqMode === 'fast-fail') {
        await this.seqStore.cancelSlot(taskId, input.clientId, input.clientSeq)
        throw new SeqGapError(expected, input.clientSeq)
      }
      const signal = await this.waitForSignal(taskId, input.clientId, input.clientSeq, timeout)
      if (signal === 'timeout') {
        const cancel = await this.seqStore.cancelSlot(taskId, input.clientId, input.clientSeq)
        if (cancel === 'already_triggered') {
          return this.handleTriggered(taskId, input)
        }
        throw new SeqTimeoutError(input.clientSeq)
      }
      const event = await this._emit(taskId, input)
      const advance = await this.seqStore.advanceAfterEmit(taskId, input.clientId, input.clientSeq)
      if (advance.triggerNext) this.notifySlot(taskId, input.clientId, advance.triggerNext)
      return event

    case 'reject_stale':
      throw new SeqStaleError(input.clientSeq, result.expected)
    case 'reject_duplicate':
      throw new SeqDuplicateError(input.clientSeq)
  }
}
```

### HTTP 层

发布端（`POST /tasks/:taskId/events`）透传 `clientId`、`clientSeq`、`seqMode`。

查询端新增路由：

```
GET /tasks/:taskId/seq/:clientId
→ 200: { clientId: string, expectedSeq: number }
→ 404: 该 clientId 未初始化
```

错误码映射：

| 情况 | 状态码 | body |
|------|--------|------|
| 正常接受（含 hold 后成功） | 201 | `{ event }` |
| seq 过期 (N < expected) | 409 | `{ error: "seq_stale", expectedSeq, receivedSeq }` |
| seq 重复注册 | 409 | `{ error: "seq_duplicate", seq }` |
| hold 超时 | 408 | `{ error: "seq_timeout", seq, expectedSeq }` |
| fast-fail 且有 gap | 409 | `{ error: "seq_gap", expectedSeq, receivedSeq }` |
| clientId/clientSeq 只提供一个 | 400 | `{ error: "validation_error" }` |

### 配置

```typescript
interface EngineConfig {
  // ...existing
  seqHoldTimeout?: number  // 默认 30000ms
}
```

### Redis TTL

每次 `processSeq` / `advanceAfterEmit` 调用时刷新 TTL（跟 task TTL 对齐，无 TTL 默认 1h）。Lua 末尾加 `EXPIRE`。

### 终态清理

Task 进入终态（completed/failed/timeout/cancelled）时调用 `cleanupSeq(taskId)` 清除所有 seq 状态。

## Scope

### Changes

| 包 | 文件 | 变更 |
|----|------|------|
| `@taskcast/core` | `types.ts` | `PublishEventInput` / `TaskEvent` 加 `clientId`, `clientSeq`, `seqMode` 字段 |
| `@taskcast/core` | `types.ts` | `ShortTermStore` 接口加 seq 方法（`processSeq`, `advanceAfterEmit`, `cancelSlot`, `getExpectedSeq`, `cleanupSeq`） |
| `@taskcast/core` | `memory-adapters.ts` | `MemoryShortTermStore` 实现 seq 方法 |
| `@taskcast/core` | `engine.ts` | 编排逻辑：processSeq → _emit → advanceAfterEmit → notify |
| `@taskcast/core` | `schemas.ts` | Zod schema 加 refine（clientId/clientSeq 同时存在或缺失） |
| `@taskcast/server` | `routes/tasks.ts` | 发布端透传新字段，新增 `GET /tasks/:taskId/seq/:clientId` |
| `@taskcast/server` | `routes/tasks.ts` | 错误码映射（408/409） |
| `@taskcast/redis` | `short-term.ts` | Redis Lua 脚本实现 seq 方法 |
| `@taskcast/sqlite` | `short-term.ts` | 事件表加 `client_id` / `client_seq` 列 |
| `@taskcast/postgres` | `long-term.ts` | 事件表加 `client_id` / `client_seq` 列 |

### Non-changes

- `getHistory()` 排序逻辑不变 —— 排序在写入时已保证
- SSE 订阅逻辑不变
- 不使用 `clientId` / `clientSeq` 的客户端完全不受影响
- Webhook 逻辑不变

### Future Work

- WebSocket 事件发布通道（天然保序，与 clientSeq 互补）
- 复用 worker WS 连接加 publish message type

## Implementation Order

1. 类型 — `PublishEventInput` / `TaskEvent` 加字段，Zod schema 加 refine
2. `ShortTermStore` 接口 — 加 seq 相关方法签名
3. `MemoryShortTermStore` — 内存实现，写单元测试
4. Engine — 编排逻辑，用 memory adapter 测
5. HTTP 路由 — 发布端透传、查询端新路由、错误码映射
6. `RedisShortTermStore` — Lua 脚本实现，integration test
7. SQLite / Postgres adapter — 事件表加 `client_id` / `client_seq` 列
8. 终态清理 — task 状态转换 hook 调用 `cleanupSeq`

## Test Plan

| 场景 | 类型 |
|------|------|
| 顺序到达 → 正常 accept | unit |
| 乱序到达 → hold → gap 补齐 → 链式触发按序 emit | unit |
| 多重 gap（2、4 缺，3、5、6 等待） | unit |
| hold 超时 → 取消注册 → 408 | unit |
| 超时与触发竞态 → already_triggered → 成功 | unit |
| fast-fail 模式 → 立即 409 | unit |
| 重复 seq → reject_duplicate | unit |
| 过期 seq → reject_stale | unit |
| 无 clientId/clientSeq → 走原逻辑 | unit |
| 首次事件初始化 expectedSeq | unit |
| clientId/clientSeq 只提供一个 → 400 | unit |
| 终态清理 seq 状态 | unit |
| 查询 expectedSeq API | unit |
| Redis Lua 原子性（并发 processSeq） | integration |
| Redis TTL 刷新与过期 | integration |
| 跨实例通知（Redis pub/sub） | integration |