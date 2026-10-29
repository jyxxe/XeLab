# TaskEngine 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## TaskEngine.claim

`async fn claim(ctx: &AuthContext, request: ClaimInput) -> Result<ClaimResult, PlatformError>`

期限上限：10000ms。事务：锁 ready 步骤 SKIP LOCKED，过期尝试转态，fence++。

领取前检查执行委托及依赖，不存在可领取记录返回 attempt=null。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `worker_id` | `string` | 内核 worker UUID |
| `lease_seconds` | `integer` | 见引用类型的完整约束 |

输出 `ClaimResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## TaskEngine.commit

`async fn commit(ctx: &AuthContext, request: AttemptCommitInput) -> Result<TaskStep, PlatformError>`

期限上限：10000ms。事务：验证当前 fence/租约/授权；提交结果及下游 outbox。

旧 fence 或过期租约拒绝；同一合法终态结果可幂等确认。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `attempt_id` | `string` | 尝试 ID |
| `execution_id` | `string` | 执行 ID |
| `fence` | `integer` | 当前租约 fence |
| `state` | `['succeeded', 'failed', 'unknown']` | 见引用类型的完整约束 |
| `output` | `JsonValue / null` | 见引用类型的完整约束 |
| `error` | `ErrorDetail / null` | 见引用类型的完整约束 |

输出 `TaskStep` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

