# Taskcast Terminal Retention Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Redis + PostgreSQL 部署提供按任务类型默认、任务级覆盖的终态历史保留机制，支持事件到期和整任务到期。

**Architecture:** 创建任务时冻结有效 cleanup 规则；持久化状态驱动后台到期扫描。复用现有归档释放、锁、creation_token 和写入者就绪检查，清理两层存储并阻止迟到异步写入；TS 与 Rust 同任务交付同一契约。

**Tech Stack:** TypeScript、Rust、PostgreSQL、Redis、Hono/Axum、Vitest、cargo test、Testcontainers。

**Spec:** [已确认设计](../specs/2026-09-28-taskcast-retention-design.md)。本计划保存在 WonderAgent 设计 worktree；Task 1–5 的代码路径均相对于 Taskcast 仓库。开始执行时将批准的设计和本计划复制到 Taskcast 的隔离分支，并保持单一版本。

## Global Constraints

- 首版仅终态，从 completedAt 计时；无可信结束时间跳过。未配置不删除，不以 TTL 或 updatedAt 代替保留期限。
- 任务 cleanup 完整覆盖全局匹配规则；空数组显式禁用。新任务冻结规则，旧任务不自动接管，全局变更不追溯。
- 自动清理默认关闭；仅支持 target events/all，启用时明确拒绝 eventFilter 和 target task。停用时保留原读写兼容。
- 支持 Redis + PostgreSQL；Memory 适配器实现确定性测试。其他组合只有满足全部能力契约才可启用，否则启动时报错，不假装已经清理。
- Node.js/Rust 的模型、配置、HTTP 行为及恢复机制同时更新；SQL 迁移以根目录 migrations/postgres 为源，重新生成 CLI 内嵌迁移。
- 后台沿用现有生命周期 tick/lease，默认每 5 秒、每 tick 最多领取 100 个任务，每任务每 tick 最多删除 1,000 条事件；不引入外部调度系统。
- 不用 flushdb、truncate 或生产库测试，不删除旧迁移、不降低覆盖率、不跳过恢复与身份校验。
- 本计划不执行上线或存量删除。默认应用规则为搜索 completed/cancelled 24 小时、failed 7 天、timeout 不删、任务记录保留。

## Review Focus

结合核心需求、实际使用场景和现有运行约束审查，避免过度设计；下面五项都在任务测试中落地：

1. 未启用/历史 cleanup JSON 不会在升级后意外删除数据（Task 1、5）。
2. 迟到写入不能复活已删事件，也不能污染同 ID 重新创建或导入的任务（Task 2、3、5）。
3. Redis 已释放但 PostgreSQL 删除中断时可继续，cold 任务不会漏扫（Task 2、3）。
4. 终态 outbox、assignment 或归档仍有工作时不得清理；不就绪任务不饿死后续任务（Task 2、3）。
5. 过期历史在 REST/SSE/archive 中含义一致，旧客户端仍能获得最终结果（Task 4、5）。

## 文件与职责

- `packages/core/src/cleanup-policy.ts` / `rust/taskcast-core/src/cleanup_policy.rs`：纯策略解析、验证和到期计算。
- `packages/core/src/cleanup-coordinator.ts` / `rust/taskcast-core/src/cleanup_coordinator.rs`：任务领取、归档释放、删除推进及错误恢复。
- `packages/postgres/src/cleanup-store.ts` / `rust/taskcast-postgres/src/cleanup_store.rs`：领取和有界清理 SQL；现有 long-term.ts/store.rs 负责接入、映射及所有写入入口的保护。
- `packages/server/src/index.ts` / `rust/taskcast-server/src/app.rs`：现有生命周期后台任务接入，不重构无关服务。
- 新建 `migrations/postgres/006_terminal_retention.sql`：任务最小策略/进度字段与到期索引，不复制事件或新增长期诊断表。

## Task 1: 策略契约和新任务冻结

**Files:** 新建上述 cleanup-policy 文件；修改 TS `types.ts`、`config.ts`、`engine.ts`、`index.ts`，Rust 对应 `types.rs`、`config.rs`、`engine.rs`、`lib.rs`，服务端 schemas/CreateTask 校验和 server-sdk 类型。测试新增 `packages/core/tests/unit/cleanup-policy.test.ts`、`rust/taskcast-core/tests/cleanup_policy.rs`，更新已有 config/engine/CLI 配置测试。

**Interfaces:**

```ts
type ResolvedCleanupConfig = { enabled: boolean; rules: CleanupRule[] };
type TaskCleanupPolicy = { rules: CleanupRule[] };
resolveCleanupConfig(config: TaskcastConfig, env: Record<string, string | undefined>): ResolvedCleanupConfig;
resolveTaskCleanupPolicy(type: string | undefined, override: TaskCleanupPolicy | undefined,
  config: ResolvedCleanupConfig, now: number):
  { cleanup: TaskCleanupPolicy; cleanupPolicyVersion: 1; cleanupResolvedAt: number } | undefined;
cleanupDeadline(task: Task, target: 'events' | 'all'): number | null;
```

Task 增加可选 `cleanupPolicyVersion: 1`、`cleanupResolvedAt: number`、`historyExpiredAt: number`。这些是服务端生成字段，create/import 的调用者不能伪造接管标记。全局 `cleanup.enabled` 默认 false，环境变量 `TASKCAST_CLEANUP_ENABLED` 可覆盖布尔值；其他策略来自配置文件。

- [ ] **Step 1 — 写失败用例。** 断言 false 默认、true/false 环境覆盖、非法布尔失败；类型通配符、无类型、任务空规则、整数组覆盖；多规则最早期限；完成时间 1,000 加 86,400,000 得 86,401,000，failed 加 604,800,000；非终态/无 completedAt 返回 null。启用时拒绝 eventFilter、task、负数/非有限/溢出期限，缺省 afterMs 为 0；关闭时兼容原 JSON。

  测试 `uses completion time only for enrolled terminal tasks` 的核心断言（enrolledTask 为 version=1、completedAt=1,000 且 events afterMs=86,400,000 的完成任务）：

  ```ts
  expect(cleanupDeadline(enrolledTask, 'events')).toBe(86_401_000);
  expect(cleanupDeadline({ ...enrolledTask, status: 'running' }, 'events')).toBeNull();
  expect(cleanupDeadline({ ...enrolledTask, completedAt: undefined }, 'events')).toBeNull();
  expect(cleanupDeadline({ ...enrolledTask, cleanupPolicyVersion: undefined }, 'events')).toBeNull();
  ```
- [ ] **Step 2 — 验证红灯。** `pnpm --filter @taskcast/core exec vitest run tests/unit/cleanup-policy.test.ts`；`cargo test --manifest-path rust/Cargo.toml -p taskcast-core --test cleanup_policy`。新增模块/行为尚不存在，应失败。
- [ ] **Step 3 — 实现纯策略与引擎接入。** 创建时且仅 enabled=true 时冻结匹配规则并打 version=1；cleanupDeadline 必须检查 version=1 才计算期限，不回填旧任务、不在 GET/transition 时重解析全局规则。已有 task.cleanup 保留原始兼容路径，客户端不能直接设置系统标记。server-sdk 继续使用已有 cleanup 输入，不扩大任意字段写权限。
- [ ] **Step 4 — 运行策略/配置/创建定向测试。** 除 Step 2 命令外运行 core 既有 config/create 相关测试及 TS/Rust 编译检查；证明改变全局规则不改变已创建任务快照。无存储清理能力时尚不开启后台删除。
- [ ] **Step 5 — 提交。** `feat: resolve terminal cleanup policies at task creation`。

## Task 2: 持久化进度、有界删除与迟到写入保护

**Files:** 新建 006 迁移、TS/Rust cleanup-store；修改 `packages/postgres/src/long-term.ts`、`rust/taskcast-postgres/src/store.rs`、其模块导出，core 的存储 trait/Memory 适配器、TS/Rust engine 异步归档入口；重新生成 `packages/cli/src/generated-migrations.ts`。新增 `packages/postgres/tests/integration/cleanup-store.test.ts`、`rust/taskcast-postgres/tests/cleanup_store.rs`；扩展 migration compatibility 测试。

**Interfaces:**

```ts
type CleanupTargetV1 = 'events' | 'all';
type CleanupClaim = {
  taskId: string; creationToken: string; claimToken: string;
  target: CleanupTargetV1; completedAt: number; taskVersion: number;
};
type DurableWriteContext = { creationToken: string };
// LongTermStore 上均为可选能力；enabled 时必须完整实现。
claimCleanupTasks(limit: number, claimTtlMs: number): Promise<CleanupClaim[]>;
renewCleanupClaim(claim: CleanupClaim, claimTtlMs: number): Promise<boolean>;
deferCleanupClaim(claim: CleanupClaim, retryAfterMs: number): Promise<void>;
beginTaskCleanup(claim: CleanupClaim, storageEpoch: number, throughIndex: number): Promise<boolean>;
deleteTaskCleanupBatch(claim: CleanupClaim, eventLimit: number):
  Promise<{ deletedEvents: number; complete: boolean }>;
```

复用已持久化的 `creation_token` 区分任务创建批次，加入内部 TaskStorageMetadata/HotWriteToken，随提交事件传到异步持久化，不暴露为 SSE 事件内容。saveTask/saveEvent/replaceLastSeriesEvent/accumulateSeries 的存储调用增加可选 `DurableWriteContext`（Rust 用 Option）。有接管标记的任务必须有正确 context；导入和创建的受控入口单独处理新创建批次。启用清理要求适配器支持带非空 creation_token 的创建领取流程，不能回退到无 token 的旧 createTaskIfAbsent。异步写入必须携带提交时取得的 token，不能执行持久化时再查询同 ID 的当前 token。

迁移增加 `cleanup_policy_version`、`cleanup_resolved_at`、`cleanup_due_at`、`cleanup_next_attempt_at`、`cleanup_claim_token`、`cleanup_claim_until`、`cleanup_target`、`cleanup_in_progress`、`history_expired_at`、`history_expired_through_index`。旧行 version 默认 NULL，不能进入队列。终态持久化时原子计算下一期限，保持已有 TTL/outbox 事务；事件清理完成后如果还有 all 期限则继续调度。

- [ ] **Step 1 — 写失败集成测试。** 覆盖旧行迁移不接管、到期/cold 领取、SKIP LOCKED 多领取者、失败任务退避让后续任务被领取；已投影/未投影 outbox 和 assignment 有无的差异。以 1,501 条事件验证每批上限 1,000，并保留结果与最高索引。
- [ ] **Step 2 — 运行红灯。** `pnpm --filter @taskcast/postgres exec vitest run tests/integration/cleanup-store.test.ts`；`cargo test --manifest-path rust/Cargo.toml -p taskcast-postgres --test cleanup_store`。使用独立 Testcontainers/专用测试 schema，不能指向已登录线上库。
- [ ] **Step 3 — 实现领取和删除。** 在行锁下复核创建批次、claim、状态/版本、completedAt、due time、cold 状态及依赖。稳定索引扫描到期且可重试的记录；失败至少延迟一个 tick。begin 原子固定清理范围并标记历史过期，读路径不能暴露删除中的部分历史。已经 history_expired 但 cleanup_in_progress 的任务继续进入领取范围，直至物理完成。每次删除最多 eventLimit 条；尾批一并清除持有正文的 series 状态、已完成 outbox 与不再有效的归档收据。all 在关联处理完成后删除任务行；物理完成前 complete=false。
- [ ] **Step 4 — 封住所有旧写入入口。** 延迟 saveEvent、latest、accumulate、saveTask 必须在数据库事务中锁任务并检查相同 creation_token 和已清理范围；已删除任务只能由正式创建/导入接口重建，普通 saveTask 不得 upsert 复活。对新创建批次使用新 token，旧异步写入即使 taskId 相同也被拒绝。清理后 `getLastEventIndex` 仍返回保留的最高使用索引，不因空事件表回到 -1。
- [ ] **Step 5 — 验证竞态与失败。** 暂停异步归档，先清理再恢复该写入，断言事件数保持 0；all 后以同 ID 正式创建新任务，旧 token 写入不得污染新任务。模拟事务回滚、claim 丢失、清理进度中断、series 与 archive 旁路恢复。`pnpm --filter @taskcast/cli generate-migrations` 后检查生成文件同步，TS/Rust 相同断言全过。
- [ ] **Step 6 — 提交。** `feat: persist guarded terminal retention cleanup`。

## Task 3: 清理协调器和后台生命周期接入

**Files:** 新建 core cleanup-coordinator TS/Rust；修改 core engine/exports、Memory 适配器、`packages/server/src/index.ts`、`rust/taskcast-server/src/app.rs`、两个 CLI start 文件。测试新增 `packages/core/tests/integration/terminal-cleanup.test.ts`、`rust/taskcast-core/tests/terminal_cleanup.rs`，扩展两端 storage-lifecycle-worker/startup 测试。

**Interfaces:** `CleanupCoordinator.sweep(limit: number, eventBatchSize: number, claimTtlMs: number): Promise<CleanupSweepResult>`；`CleanupSweepResult = { claimed: number; completed: number; deferred: number; failed: number; deletedEvents: number }`；`TaskEngine.sweepCleanup(limit = 100, eventBatchSize = 1000)` 调用协调器。Rust 同名 snake_case 与等价数据结构。

- [ ] **Step 1 — 写失败测试。** enabled=false 无扫描；enabled=true 且缺适配能力启动失败；每 tick 上限生效；事件过期但后台尚未物理完成时不能计入 completed；active/paused/blocked/assigned/pending 均不清理。
- [ ] **Step 2 — 验证红灯。** `pnpm --filter @taskcast/core exec vitest run tests/integration/terminal-cleanup.test.ts`；`cargo test --manifest-path rust/Cargo.toml -p taskcast-core --test terminal_cleanup`。
- [ ] **Step 3 — 接入安全流程。** PG claim 后读取业务无关的 Taskcast 状态，先调用现有 releaseTaskStorageAtCurrentDurableIndex，完成后重新取锁/核验再 begin/delete。不能持有同一 Redis 锁再递归调用会取锁的 release 方法；长归档期间续 claim，失去 claim 后不得继续删除。cold 任务直接校验，不重复归档；任何未完成关联工作先 defer。
- [ ] **Step 4 — 接入后台与协议。** 复用生命周期 tick 的间隔/任务批量，事件批量固定默认 1,000。新写入者公布存储协议版本 3；永久清理要求全部存活写入者至少 v3，现有纯 Redis 释放仍沿用其原能力要求，不因本功能关闭而退化。检查通过只表示能力就绪，不替代删除前的数据校验。
- [ ] **Step 5 — 恢复测试。** Redis 释放后 PG 故障、半批完成重启、lease/claim 丢失、两个清理者竞争、旧 v2 写入者存活、cold 任务不在 Redis 列表等场景不误删、不复活、不饿死后续任务。只输出安全摘要日志，不保存逐次扫描日志。运行新增 core 测试和两端 lifecycle/startup 定向测试，退出码 0。
- [ ] **Step 6 — 提交。** `feat: run bounded terminal cleanup in lifecycle workers`。

## Task 4: 历史过期查询、SSE 和归档兼容

**Files:** 修改 TS/Rust engine、tasks/sse 路由、Task schema/OpenAPI 和错误映射，`packages/client/src/client.ts`、server-sdk 测试。新增 `packages/server/tests/cleanup-routes.test.ts`、`rust/taskcast-server/tests/cleanup_routes.rs`；扩展 archive-routes、client 和 server-sdk 测试。

**Interfaces:** GET task 的可选 `historyExpiredAt` 为毫秒时间戳。历史查询保持数组响应，过期返回 []，附 `X-Taskcast-History-Expired: true`；SSE 顺序为 `taskcast.history_expired`（data 含 taskId、expiredAt）再 `taskcast.done`（保留原终态 reason）。新增可选 client callback `onHistoryExpired?: (info: { taskId: string; expiredAt: number }) => void`，旧调用者不必提供。导出错误码 `TASKCAST_HISTORY_EXPIRED`、HTTP 409；整任务已删除仍 404。

- [ ] **Step 1 — 写失败 HTTP 用例。** 无身份/权限仍先按原方式返回 401/403；过期标记、空历史和 SSE done 一致，传旧 since.id/index/timestamp 也明确告知过期；未过期响应保持原状；旧客户端忽略新增事件后仍收到 done。
- [ ] **Step 2 — 验证红灯。** `pnpm --filter @taskcast/server exec vitest run tests/cleanup-routes.test.ts`；`cargo test --manifest-path rust/Cargo.toml -p taskcast-server --test cleanup_routes`。
- [ ] **Step 3 — 实现读路径。** 引擎从持久化清理标记决定可见历史，不能从残留 Redis、series 或归档旁路重新拼出已过期事件。global SSE 的历史读取和单任务 SSE 使用同一判定；只发送现有终态 done，不重新执行任务。CORS 暴露新增 header，SDK 类型保持向后兼容。
- [ ] **Step 4 — 固定导入/导出语义。** 过期任务不得生成完整 archive；普通导入不隐式覆盖已清理历史。显式 overwrite 继续按现有授权、archive 校验和存储锁处理，在受控事务中分配新 creation_token，旧异步写入不得进入新创建批次；系统清理标记不能由归档内容伪造。导入任务按历史任务处理：保留 cleanup JSON，但服务端重置接管及清理进度字段，不因当前全局 enabled 或归档中的旧 deadline 立即删除恢复的历史；再次接管遵循存量 allowlist 流程。
- [ ] **Step 5 — 验证并提交。** 运行上述路由测试、两端 archive/SSE 回归、client/server-sdk 定向测试及类型检查；通过后提交 `feat: expose expired task history consistently`。

## Task 5: 双运行时验收、CI 和应用接入说明

**Files:** 新增 `rust/tests/terminal_cleanup_parity.test.ts`（一个场景同时驱动 Node 与 Rust 的真实 Redis/PG 服务）、`docs/guide/retention.md` 与 `.zh.md`；更新 concepts/deployment 英中文档、`.github/workflows/ci.yml`、`.github/workflows/rust.yml`、`.github/workflows/e2e.yml` 及 `.changeset/taskcast-terminal-retention.md`。测试辅助放同级 helpers，复用现有容器启动方式。

**Interfaces:** 公共 HTTP 契约来自 Task 4，配置与策略来自 Task 1；测试不直接修改业务状态表来伪造正常流程。只有故障注入/迁移兼容用例可明确操控测试库状态。生产删除不在本任务授权范围。

- [ ] **Step 1 — 建立共享验收场景。** 创建 running 任务，发布 keep-all/latest/accumulate 后完成；以短测试期限验证 events 和 all 两种策略、空规则覆盖、默认改变不追溯、遗留任务不接管、失败/超时差异、重启恢复。用与 WonderAgent 相同的事件 data/series 格式发布两实体各多次通知，断言每组只保留最新通知且所有实时事件均到达。
- [ ] **Step 2 — 跑跨进程 Good/Bad Case。** 使用可用随机端口和独立 Redis namespace/PG schema 启动两种运行时；历史过期结果、HTTP 状态、header、SSE 顺序、任务结果与迟到写入保护相同。执行 `pnpm exec vitest run --config rust/tests/vitest.config.ts rust/tests/terminal_cleanup_parity.test.ts`，容器/服务未启动必须失败，不能静默 skip。
- [ ] **Step 3 — 完善 CI。** 保留锁文件安装、构建、类型和全部现有测试。TS 使用既有 coverage 入口执行完整测试，不以新增目录替代旧采集范围；Rust 保留现有独立检查/构建/测试/coverage 并加入新场景。按 Taskcast AGENTS.md 核验 PR 覆盖率要求并让 CI 实际阻塞未达标；既有非本改动覆盖缺口需如实报告，不能私自排除文件或降低阈值。迁移生成检查继续执行。
- [ ] **Step 4 — 更新文档和发布说明。** 说明 enabled 默认关闭、终态计时、whole-policy 覆盖、events/all、首版不支持 eventFilter/task、关闭开关不恢复历史、只接管新任务。给出已确认的 WonderAgent 24 小时/7 天示例，但不把它做成所有 Taskcast 部署的内置默认值。加入独立的 Redis 释放说明，避免混淆两者。
- [ ] **Step 5 — 记录存量接入和上线流程。** 先备份、升级全部 writer、检查能力再启用；存量另行只读预览并核对业务库、allowlist 与状态，未明确接管的旧任务不被删除。本版不提供自动接管全量存量的快捷开关；预览和正式接管作为另一次授权的运维工作。
- [ ] **Step 6 — 验证与提交。** 本地复用同状态已通过的定向结果；最终由 CI 执行完整门禁。`pnpm --filter @taskcast/cli generate-migrations`、`pnpm build`、`pnpm lint`、定向 parity 均需有真实结果。changeset 遵循 fixed versioning，不手动改 tag/版本。提交 `test: verify terminal retention across runtimes`。

## 执行顺序和完成定义

推荐主 Agent 顺序完成 Task 1–5，每个任务同时实现 TS/Rust；接口耦合较强，不把两种语言交给互不沟通的实现者。最后由独立 reviewer 按业务需求和运行约束审查整个分支。通知合并计划可先完成并单独验证，不必等待本计划全部完成。

完成意味着：产品代码、迁移、配置、文档、双运行时测试及 CI 就绪；不等于已经发布新 Taskcast 镜像或启用线上清理。合并、发布、部署和存量清理按用户实际授权继续，不能从设计批准推断生产删除授权。

## Self-Review

已把策略、创建标记、整份历史/任务期限、存储恢复、迟到写入、读取/导入/导出兼容、旧客户端、配置关闭、迁移、双运行时及存量流程映射到具体任务。接口统一使用 CleanupClaim、DurableWriteContext 和 historyExpiredAt；任务之间不另造并行命名。五个 Review Focus 都有对应测试。
