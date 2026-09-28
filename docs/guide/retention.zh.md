# 终态历史保留

永久清理默认关闭（`cleanup.enabled: false`）。启用需要支持存储协议 3 的 Redis、PostgreSQL 适配器，并确保所有存活写入者已升级。Memory 适配器支持测试；SQLite 或能力不完整的自定义适配器会在启用时拒绝启动。

Redis 热数据释放与永久清理是两个独立功能：前者先归档，再释放 Redis，不删除 PostgreSQL 历史；后者删除持久化历史或整个任务。关闭清理开关会停止后续清理轮次，不能恢复已经删除的历史。

## 策略

启用后，服务器在创建任务时快照保存匹配的全局 `cleanup.rules`。任务自己的 `cleanup.rules` **整份覆盖**全局规则；`rules: []` 表示不清理。修改全局默认值不会追溯修改已有任务。旧任务、旧 cleanup JSON、导入的归档都不会自动接管。

只有 `completed`、`failed`、`cancelled`、`timeout` 终态可以进入清理。期限从 `completedAt` 计算，不从创建时间或最后事件时间计算。没有可信完成时间的任务跳过；`pending`、`assigned`、`running`、`paused`、`blocked` 不删除。

- `target: events`：删除整份事件历史，保留状态、结果、错误、元数据。
- `target: all`：连同任务记录一起删除。同时匹配时，最早的 events 期限生效，all 是整个任务的最终保留上限。
- `trigger.afterMs`：完成后的非负毫秒数；省略表示立即具备清理资格。
- `match.taskTypes`、`match.status`：可选筛选。首版启用时拒绝 `eventFilter` 和 `target: task`。

以下仅为 **WonderAgent** 示例，不是 Taskcast 内置默认值：

```yaml
cleanup:
  enabled: true
  rules:
    - name: search-success-events
      match:
        taskTypes: [influagent.search.youtube]
        status: [completed, cancelled]
      trigger: { afterMs: 86400000 }
      target: events
    - name: search-failure-events
      match:
        taskTypes: [influagent.search.youtube]
        status: [failed]
      trigger: { afterMs: 604800000 }
      target: events
```

完成、取消的历史保留 24 小时，失败历史保留 7 天；超时历史和任务记录不自动删除。环境变量 `TASKCAST_CLEANUP_ENABLED=true|false` 优先于文件开关，其他值会导致启动失败。

## 查询、订阅与恢复

开始清理后，`GET /tasks/:id` 返回可选的 `historyExpiredAt`（Unix 毫秒）。历史接口返回 `[]`，并带 `X-Taskcast-History-Expired: true`；启用 CORS 时会暴露此 header。即使物理删除尚未完成，也不会返回残缺历史。

SSE 先发送 `taskcast.history_expired`，数据为 `{taskId, expiredAt}`，再发送携带原终态 reason 的 `taskcast.done`。旧游标重连也会收到过期通知。浏览器客户端可选传入 `onHistoryExpired(info)`；旧调用方可以忽略新事件。全局 SSE 保持订阅新建任务实时事件的原行为，不补放历史任务。

导出已过期历史返回 HTTP 409，错误码 `TASKCAST_HISTORY_EXPIRED`；整个任务删除后返回 404。普通归档导入仍不覆盖已有任务。显式 `overwrite: true` 沿用任务管理权限和归档校验，在存储锁内生成新的创建标识，拒绝旧创建批次的迟到写入。归档中的系统清理标记会被移除；cleanup JSON 作为旧配置保留，但恢复的历史不会立即自动清理。

## 运行与上线

后台默认每 5 秒检查一次，每轮最多领取 100 个任务，每任务每轮最多删除 1,000 条事件。间隔和任务批量沿用 `storageLifecycle.ttlSweepIntervalSeconds`、`ttlSweepBatchSize`。忙碌或失败的任务延后重试，让后续任务继续处理；进程重启后会从 PostgreSQL 继续处理已经不在 Redis 的任务。

清理先解决归档与释放状态、释放 Redis，再复核创建标识、完成时间和状态。尚未结算的 assignment、终态投影及未完成归档会延后处理。先提交逻辑过期标记，再分批物理删除，同时清除 series 正文和过时归档收据；仅清历史时保留已使用的最高事件索引。stdout 只输出汇总数量和关联标识，不输出事件正文。

1. 备份 PostgreSQL，确认业务结果、来源证据、账单或恢复必需数据保存在业务系统或保留的任务记录中。
2. 执行增量迁移，包括 `006_terminal_retention.sql`。
3. 升级全部 API、Worker、嵌入式写入者至存储协议 3。旧写入者存活时永久清理会等待；普通 Redis 释放仍允许协议 2。
4. 在非生产环境创建可丢弃的新任务，验证过期、重启和归档恢复，按任务类型审核策略。
5. 仅在已批准的环境开启开关并观察生命周期汇总。CI 通过不代表线上已开启清理。

存量接管另行处理：先按类型、状态、年龄做只读清单，核对业务引用与恢复要求，再准备备份和明确的任务 allowlist。本版不提供全量自动接管开关；迁移不会删除或接管旧行。修改策略或完成代码实现，不等于授权清理线上数据。
