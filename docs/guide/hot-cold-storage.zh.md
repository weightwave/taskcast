# 冷热存储

[English](hot-cold-storage.md)

Taskcast 可以把 Redis 作为有界的活跃任务与重放层，同时让 PostgreSQL
成为持久化事实源。只有在带 fencing 的归档协议证明 PostgreSQL 已完整覆盖
Redis 事件区间后，Taskcast 才会释放热数据。冷任务后续发生写入时只恢复有界
重放窗口；历史查询仍以 PostgreSQL 的完整记录为准。

## 旧机制为什么会持续涨内存

旧的三层写入路径并没有真正实现冷热分层：

- Redis 虽然叫短期存储，但任务和事件 key 没有在 PostgreSQL 追平后安全释放
  的生命周期。
- PostgreSQL 只是接收长期异步双写；异步双写不等于删除屏障，系统没有持久化
  watermark 或归档回执去证明每条 Redis 事件都已落库。
- 历史读取只要从 Redis 读到任何事件，就优先返回 Redis。Redis 中残留的局部
  前缀或重放尾部可能遮住 PostgreSQL 的完整历史。
- 任务 `ttl` 只是让 Redis key 过期，并不会持久化地把任务迁移到 `timeout`、
  只生成一次 timeout 事件并结算 worker 所有权。

因此，请求负载低不代表 Redis 内存会下降。一个一直保持 `pending`、持续产生
重试事件的任务，可以让 Redis 事件列表无限增长。

## 存储模型

同时配置 Redis 和支持冷热协议的 PostgreSQL adapter 后：

- PostgreSQL 是任务历史与持久化生命周期元数据的事实源。
- Redis 只保存活跃任务状态、当前写 fence、series 状态和有界重放窗口。
- 读取冷任务不会触发回热；历史由 PostgreSQL 与已证明安全的热尾部合并。
- 冷任务发生变更时，系统先获取存储 lease，再恢复任务、持久化 series 状态、
  下一个全局 index，以及最多 `rehydrateReplayEvents` 条最近事件，然后提交
  新写入。
- 释放和回热前后，事件 index 始终单调递增。

协议使用按任务 lease、storage epoch、关闭写入的 fence、归档 generation、
批次回执、manifest 和持久化 archive watermark。只有 watermark 等于被 fence
锁定的 Redis high watermark 后，Redis 任务 key 才能删除。归档失败或结果
不确定时会保留 Redis，由恢复/重试机制继续处理。

## 释放热存储

释放操作是显式的：

```http
POST /tasks/{taskId}/storage/release
Content-Type: application/json

{
  "expectedLastEventIndex": 1542,
  "inactiveSince": 1785168000000
}
```

两个字段都是整数前置条件，`inactiveSince` 是毫秒 Unix 时间戳。必须从权威的
任务/历史快照读取，不能猜测。调用方需要 `task:manage` 权限。

成功响应：

```json
{
  "taskId": "01...",
  "storageState": "cold",
  "archiveWatermark": 1542,
  "released": true
}
```

对已经 cold 的任务再次调用是幂等的，返回 `released: false`。

重要错误：

- `409 storage_precondition_failed`：任务出现了更新的活动，或最后事件 index
  已改变。重新读取任务/历史后再决定。
- `409 storage_busy`：另一个释放、回热或冲突写入持有生命周期 fence。使用
  有界退避重试。
- `500 storage_integrity_error`：来源、manifest、回执覆盖或 watermark 不一致。
  立即暂停自动释放并调查。
- `503 storage_release_unsupported`：当前 adapter 不支持该协议。
- `503 storage_unavailable`：PostgreSQL、Redis 或 writer readiness 不可用。
  Redis 会保留；修复 readiness 后由持久化请求重试。

自动 retention 只处理终态任务。非终态任务必须由真正的 owning service 在释放
session/task 所有权后显式调用。仅仅一段时间没有事件，并不能证明 `pending` 或
`running` 任务可以安全释放。

## 配置

TypeScript 与 Rust 服务支持完全相同的配置。环境变量优先于配置文件中的
`storageLifecycle`。

| 环境变量 | 配置项 | 默认值 | 含义 |
| --- | --- | ---: | --- |
| `TASKCAST_HOT_RETENTION_ENABLED` | `hotRetentionEnabled` | `false` | 只自动释放符合条件的终态任务。 |
| `TASKCAST_HOT_RETENTION_TERMINAL_SECONDS` | `hotRetentionTerminalSeconds` | `86400` | 终态任务自动释放前的宽限期。 |
| `TASKCAST_HOT_RETENTION_IDLE_SECONDS` | `hotRetentionIdleSeconds` | `3600` | 持久化 release cutoff 进入 worker 重试前的最小年龄。 |
| `TASKCAST_REHYDRATE_REPLAY_EVENTS` | `rehydrateReplayEvents` | `1000` | 后续写入前恢复到 Redis 的最近事件数。 |
| `TASKCAST_STORAGE_LOCK_TTL_SECONDS` | `storageLockTtlSeconds` | `30` | 存储 lease 与 claim 的时长。 |
| `TASKCAST_TTL_SWEEP_INTERVAL_SECONDS` | `ttlSweepIntervalSeconds` | `5` | 生命周期 worker 扫描间隔。 |
| `TASKCAST_TTL_SWEEP_BATCH_SIZE` | `ttlSweepBatchSize` | `100` | 每次扫描最多 claim 的任务数。 |

除 `TASKCAST_HOT_RETENTION_ENABLED` 外，其余值必须是正整数。服务会在启动时
拒绝非法值以及秒转毫秒溢出。YAML 示例：

```yaml
storageLifecycle:
  hotRetentionEnabled: false
  hotRetentionTerminalSeconds: 86400
  hotRetentionIdleSeconds: 3600
  rehydrateReplayEvents: 1000
  storageLockTtlSeconds: 30
  ttlSweepIntervalSeconds: 5
  ttlSweepBatchSize: 100
```

只有长期存储 adapter 声明支持 durable TTL 时，持久化 TTL 扫描才会启动。
自动 retention 默认保持关闭，除非显式启用。

## TTL 与恢复 worker

生命周期 worker 会分别、有界地执行：

- 持久化 TTL claim 与终态化；
- terminal projection 修复；
- 未完成 release 恢复；
- 持久化 release request 重试；
- 可选的终态 retention。

TTL 与 release 使用独立指数退避。PostgreSQL 故障不会导致 Redis 被删除。
worker 输出不包含 payload 的结构化 JSON：

- `storage_lifecycle_tick`：包含耗时，以及 TTL、projection、release request、
  retention 和有界热任务采样的计数；
- `storage_lifecycle_error`：包含失败操作、错误信息，以及在可安全提供时的
  task ID；
- `storage_release`：包含结果、耗时、源事件数/序列化字节数、前后状态、
  archive watermark，以及失败时的稳定错误码；
- `storage_rehydrate`：包含结果、耗时、重放数量、archive watermark、
  最大 event index 和 storage epoch；
- `storage_history_read`：包含 `hot`、`durable` 或 `durable+hot` 来源、
  延迟和事件数；
- `storage_watermark_mismatch`：只记录期望/实际 watermark，不记录事件内容；
- `storage_hot_task`：在有界采样中标记异常老或异常大的热任务，并记录
  age 和 event count。

这些事件只包含 task ID 和存储元数据，不包含 task/event payload。指标系统可
根据状态前后变化维护 hot/releasing/cold gauge，并用 PostgreSQL 元数据表定期
校准。`sourceBytes` 是归档源的序列化大小，可能与 Redis allocator 实际释放
字节数不同。

启用 release 前检查 `/health/detail`：`storage.releaseReady` 必须为 `true`，
`requiredStorageProtocolVersion` 必须为 `2`，且 `incompatibleWriterIds`
必须为空。

## 没有增加事件上限

本次修改没有增加 `maxEvents`、事件类型降噪或静默截断。历史 API 的 `limit`
只限制单次响应，不是 retention 策略。`TASKCAST_REHYDRATE_REPLAY_EVENTS`
只限制写入时恢复到 Redis 的重放缓存；完整权威历史仍保存在 PostgreSQL。

生产迁移和回滚门禁请遵循
[冷热存储上线 runbook](../runbooks/2026-07-16-production-hot-cold-rollout.md)。
