# Delegator 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Delegator.issue

`async fn issue(ctx: &AuthContext, request: DelegationInput) -> Result<DelegationRecord, PlatformError>`

期限上限：10000ms。事务：冻结规则、来源凭证和有效期，同事务写入。

不得扩大实际调用凭证范围；服务模式独立批准。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `subject_id` | `string` | 以谁的权限执行 |
| `rules` | `array` | 本次固定上限 |
| `expires_at` | `string` | 见引用类型的完整约束 |
| `run_as` | `['user', 'service']` | 见引用类型的完整约束 |
| `service_account_id` | `string / null` | 见引用类型的完整约束 |

输出 `DelegationRecord` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

