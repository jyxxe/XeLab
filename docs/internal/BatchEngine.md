# BatchEngine 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## BatchEngine.submit

`async fn submit(ctx: &AuthContext, request: BatchSubmitInput) -> Result<SkillBatch, PlatformError>`

期限上限：10000ms。事务：短事务锁定批次/项目，外部Skill调用与对象存储写入不持有数据库事务。

校验全部输入与固定上限；批次、项目、幂等、outbox同事务提交；返回前不执行业务。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `skill_id` | `string` | 资源唯一标识 |
| `request` | `SkillBatchRequest` | 见引用类型的完整约束 |

输出 `SkillBatch` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `OUTCOME_UNKNOWN`, `RESOURCE_EXHAUSTED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## BatchEngine.claim

`async fn claim(ctx: &AuthContext, request: BatchClaimInput) -> Result<BatchClaimResult, PlatformError>`

期限上限：10000ms。事务：短事务锁定批次/项目，外部Skill调用与对象存储写入不持有数据库事务。

复用SKIP LOCKED/30s租约/10s续约/fence规则；先检查授权、期限与并行配额；unknown不领取。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `worker_id` | `string` | 资源唯一标识 |
| `maximum` | `integer` | 本次最多领取数 |

输出 `BatchClaimResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `OUTCOME_UNKNOWN`, `RESOURCE_EXHAUSTED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## BatchEngine.renew

`async fn renew(ctx: &AuthContext, request: BatchLeaseInput) -> Result<BatchLeaseInput, PlatformError>`

期限上限：10000ms。事务：短事务锁定批次/项目，外部Skill调用与对象存储写入不持有数据库事务。

在有效租约和当前授权下续约；失效fence不能续命；不改变业务deadline。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `batch_id` | `string` | 资源唯一标识 |
| `item_id` | `string` | 调用方关联键；不是行ID或授权凭证 |
| `execution_id` | `string` | 资源唯一标识 |
| `fence` | `integer` | 当前fence |

输出 `BatchLeaseInput` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `OUTCOME_UNKNOWN`, `RESOURCE_EXHAUSTED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## BatchEngine.commit

`async fn commit(ctx: &AuthContext, request: BatchCommitInput) -> Result<SkillBatchItem, PlatformError>`

期限上限：10000ms。事务：短事务锁定批次/项目，外部Skill调用与对象存储写入不持有数据库事务。

校验租约/fence/当前授权与结果契约，CAS提交项状态、汇总计数及outbox；迟到结果核实但不覆盖。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `lease` | `BatchLeaseInput` | 见引用类型的完整约束 |
| `state` | `string` | 仅合法结果 |
| `result` | `SkillCallResult / null` | 见引用类型的完整约束 |
| `error` | `ErrorDetail / null` | 见引用类型的完整约束 |

输出 `SkillBatchItem` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `OUTCOME_UNKNOWN`, `RESOURCE_EXHAUSTED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

