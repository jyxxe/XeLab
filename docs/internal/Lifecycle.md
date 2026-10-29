# Lifecycle 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Lifecycle.reconcile

`async fn reconcile(ctx: &AuthContext, request: ReconcileInput) -> Result<ReconcileResult, PlatformError>`

期限上限：60000ms。事务：短事务记录进度→外部操作→短事务写观测/outbox。

revision 过时重新排队；数据库连接失效停止提交。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `deployment_id` | `string` | 目标部署 |
| `expected_revision` | `integer` | 期望 revision |

输出 `ReconcileResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

