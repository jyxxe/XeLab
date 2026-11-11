# M5 差距矩阵：Task / Workflow / Schedule 与现有 M4 原语

[实施索引](README.md) · [任务执行规范](../tasks/execution.md) · [恢复规范](../tasks/recovery.md) ·
[Workflow 规范](../tasks/workflows.md) · [Schedule 规范](../tasks/schedules.md) ·
[Batch 规范](../tasks/batches.md) · [路线图](roadmap.md) · [Kernel 实现说明](../../crates/kernel/README.md)

> 研究基线：main `44aac50407fb420252023121afde4a2146f6918c`。本文只读对照源码、
> 基线与契约，不修改 Kernel、SQL 或 Proto。结论均附代码路径或契约/文档链接；
> 没有源码证据的项标 `unknown`，不臆造完成状态。M4 事件是至少一次投递，不是业务调度。

## 判定口径

- **已实现**：源码路径存在且注册了真实入口（HTTP 路由或已接通的 RPC），并有测试。
- **未实现**：只有契约、Schema/文档，或路由统一返回明确不支持错误。
- **待验证**：有部分可复用代码路径，但缺少任务级完整语义或运行证据。
- **存储先行**：`crates/kernel/src/db.rs` 把 `docs/contracts/metadata.sql` 作为
  `BASELINE` 迁移整体应用，因此目标表已随每个数据库创建；表存在不等于能力交付。

## 结论摘要

1. M5 的 `execution_delegations`、`workflows`、`tasks`、`task_steps`、`attempts`、
   `schedules`、`schedule_fires`、`skill_batches`、`model_aliases` 表均已存在于基线
   Schema（`docs/contracts/metadata.sql`），但 `crates/kernel/src/` 中没有任何
   task/workflow/schedule/batch 引擎或处理器；`api.rs` 对未实现 operationId 统一返回
   `503 NOT_READY`（`crates/kernel/src/api.rs:937`），`crates/` 中无 `TaskWorker` 实现。
2. 任务范围依赖当前被显式拒绝：`crates/kernel/src/dependencies.rs:702` 返回
   `Task-scoped resolution requires the TaskEngine`。
3. M4 已交付可复用原语：事件 30s 租约与 HMAC 令牌摘要、过期/旧租约 fencing、退避重投、
   outbox、调用 UNKNOWN 账本与 `QueryResult`、完整来源链授权与撤销。
4. M5 P0 最小闭环应依次为 Workflow/Task 提交 → 核心执行器（SKIP LOCKED、attempt/fence/
   租约）→ 完成事务与下游就绪 → 恢复与 UNKNOWN 核实；TaskWorker 长任务、Schedule、
   Batch 与 AI 预设后置且独立。

## 差距矩阵

| 能力 | 状态 | 证据 | 差距与停止条件 |
|---|---|---|---|
| WorkflowSpec 发布、64 步图校验、不可变版本、归档 | 未实现 | 表：`metadata.sql:201`；契约：[createWorkflow](../api/operations/tasks/createWorkflow.md)、[workflows.md](../tasks/workflows.md)；`crates/` 无处理器 | 环/未知依赖/重复 ID 校验、版本递增与归档；有真实 DB 证据前不得标记 operationId 可用 |
| Task 提交与状态机（含 `waiting_dependency`、`paused_authorization`、`cancel_requested`、`unknown`） | 未实现 | 表：`metadata.sql:208` 含完整 CHECK；契约：[createTask](../api/operations/tasks/createTask.md)；路由 503 | 提交时冻结委托与 deadline、初始化步骤；`unknown` 不是终态 |
| TaskStep / Attempt / fence（SKIP LOCKED、单活动尝试、fence 单调、30s 租约、10s 心跳） | 未实现 | 表：`metadata.sql:215`、`:223`（`UNIQUE(step_id,fence)`、`attempts_one_active`、`task_steps_claim_idx`）；规范：[execution.md](../tasks/execution.md)；无执行器代码 | 领取/接管/续约/提交全链 fence 校验；可复用事件租约模式（`events.rs:660` 起） |
| 冻结执行委托（`maximum_rules`、`run_as`、服务账户） | 待验证 | 普通调用已创建空规则委托并撤销：`auth.rs:430`、`broker.rs:1005`、`api.rs:687`、`recovery.rs:79`；`host_calls.rs:122` 实时联查 | 任务提交按 workflow 冻结上限、每步复核原委托；`maximum_rules` 当前固定 `'[]'` |
| UNKNOWN 核实与步骤重试（`resolveUnknownStep`、`retryTask`、`cancelTask`） | 未实现 | 契约：[resolveUnknownStep](../api/operations/tasks/resolveUnknownStep.md)、[recovery.md](../tasks/recovery.md)；调用层已有 unknown 账本与 `getCall` | confirmed_succeeded / confirmed_not_executed 证据模型、审计、与显式 retry 的边界 |
| 副作用分类（none/idempotent/queryable/non_repeatable） | 待验证 | 调用层已按四类处理 unknown（[Kernel README](../../crates/kernel/README.md) 调用行）；任务层无按步骤汇总 | 任务级重试必须继承能力语义；次数 >1 不等于允许重复副作用 |
| 检查点（Blob 先传后报、pin、版本兼容、`checkpoint_blob_id`） | 未实现 | 表：`metadata.sql:223`；Blob GC 已引用 `attempts.checkpoint_blob_id`（`blobs.rs:618`）；无 TaskWorker/Execute | pin 生命周期、`data_schema_version`/`plugin_version` 兼容校验、跨模型不默认复用 |
| TaskWorker（Execute 流、Heartbeat、Checkpoint、Result、Cancel、Query） | 未实现 | Proto：`docs/contracts/proto/xelab/v1/task.proto`；内部契约：[TaskEngine.md](../internal/TaskEngine.md)；`crates/` 无服务实现 | 工作负载 mTLS、fence 校验、查询/去重跨容器重启持久化 |
| 重启恢复（对账过期租约与插件记录后接管或 unknown） | 未实现 | 可参考：事件恢复与调用 unknown 恢复（`recovery.rs`）；规范：[recovery.md](../tasks/recovery.md) | 恢复顺序、不得自动重放 unknown、检查点续算与步骤重试分开 |
| 取消与撤权（任务级 `cancel_requested`、`paused_authorization`、祖先链） | 未实现 | 调用取消/撤权已实现并有测试（`broker.rs`、`api.rs`）；任务层无接口 | 停止领取、通知活动插件、保留 unknown；撤权后不得偷偷续跑 |
| cron / Schedule（五字段、IANA 时区、UTC fire_at、`(schedule_id,scheduled_at)` 唯一、DST/misfire/overlap） | 未实现 | 表：`metadata.sql:233`、`:238`；规范：[schedules.md](../tasks/schedules.md)；操作：[createSchedule](../api/operations/tasks/createSchedule.md) 等 | 触发幂等、恢复先对账再 misfire、plan 改动不取消已建任务 |
| M4 事件触发任务 | 未实现（且非 M4 承诺） | M4 事件仅投递：[events.md](../tasks/events.md)、[m4-events.md](m4-events.md)；无订阅→任务创建代码 | 明确边界：事件消费者须自行提交任务；不得把 ACK 当业务调度完成 |
| BatchEngine 适配器（`skill_batches` 复用同一租约/fence/Broker） | 未实现 | 表：`metadata.sql:584`、`:596`；规范：[batches.md](../tasks/batches.md)、[BatchEngine.md](../internal/BatchEngine.md) | 与 Workflow 共用执行器但独立聚合状态；不得先于 P0 执行器交付 |
| AI 预设（EmbeddingGemma 2 预设、知识表/128 维索引、模型别名） | 未实现 | `model_aliases` 表：`metadata.sql:275`；操作：[docs/api/operations/ai](../api/operations/ai/README.md)；基线 Schema 无知识表 | 独立后置范围；不得与 P0 任务闭环捆绑，也不得据模型别名推断任务能力 |

## M5 P0 建议顺序与停止条件

1. **Workflow 与 Task 提交**（发布/读取/归档、图校验、创建任务、冻结委托、步骤初始化）。
2. **核心执行器最小闭环**（ready 步骤 SKIP LOCKED 领取、attempt/fence/租约、同步 Skill
   节点经 Broker 调用、10s 续约）。
3. **步骤完成事务**（校验授权与租约 → 输出契约 → 终态与引用 → 下游 ready → outbox）。
4. **恢复与 UNKNOWN 核实**（重启对账、过期租约接管、`resolveUnknownStep`/`retryTask`/
   `cancelTask`、审计）。
5. **TaskWorker 与检查点**（长期 `automation.execute` 节点、Heartbeat、Checkpoint pin）。
6. **Schedule cron 引擎**（与 P0 执行器共用任务提交；触发唯一键与恢复对账）。
7. **Batch 与 AI 预设**（独立范围，可并行于 5/6）。

停止条件（全部适用）：每步只将实现并验证的 operationId 标记为可用；`fence` 只保护平台
元数据提交，不替代外部系统的幂等键；`unknown` 永不自动重放；没有真实 PostgreSQL/OCI
证据的步骤停留在规范状态；不修改既有 M1–M4 契约、SQL 指纹或权限快照。

## 后续小任务候选（互不重叠，未创建子 Issue）

1. **Workflow 定义与任务提交面**：实现 `createWorkflow/getWorkflow/listWorkflows/
   archiveWorkflow` 与 `createTask/getTask/listTasks/listTaskSteps`，含 64 步图校验、
   不可变版本、冻结 ExecutionDelegation。前置：无；资源：PostgreSQL。停止条件：仅当
   这些 operationId 有真实 DB 测试后标记可用，不连带执行器。
2. **任务执行器最小闭环**：ready 步骤领取、attempt/fence/租约续约、同步 Skill 节点、
   完成事务与 outbox。前置：候选 1。停止条件：真实 PostgreSQL + 现有 Broker 集成测试；
   不包含 TaskWorker 与 cron。
3. **恢复、UNKNOWN 核实与取消**：重启对账、`resolveUnknownStep`/`retryTask`/`cancelTask`、
   任务级撤权暂停与审计。前置：候选 2。停止条件：unknown 不自动重放；补偿是新的受权步骤。
4. **TaskWorker 长任务与检查点**：`Execute/Cancel/Query`、Heartbeat、Checkpoint pin 与
   版本兼容。前置：候选 2/3；需真实 OCI 与 mTLS 工作负载证据。
5. **Schedule cron 引擎**：五字段解析、UTC `fire_at`、触发唯一键、DST/misfire/overlap 与
   恢复对账。前置：候选 1；可与候选 4 并行。停止条件：先对账再补触发，禁止补发全部历史时刻。

## 运行证据与边界

- `python3 docs/tools/validate_docs.py --strict`：本机已运行，PASS（将随 PR 附输出）。
- 本机无 Rust 工具链、特权 OCI 宿主与测试数据库，`cargo test`/DB/OCI 证据未运行；
  相关结论全部来自当前源码与契约静态核对，运行期行为标 `unknown` 的项不得据此宣称已完成。
- 本矩阵不替代 M5 正式验收，也不代表任何 M5 operationId 已可用；正式生产验收仍由
  [#50](https://github.com/jyxxe/XeLab/issues/50) 跟踪。AWS 继续 DEFERRED。