# Identity 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Identity.authenticate

`async fn authenticate(ctx: &AuthContext, request: AuthenticateInput) -> Result<AuthContext, PlatformError>`

期限上限：2000ms。事务：只读当前凭证、祖先及源链；不允许凭证缓存绕过当前状态。

凭证唯一确定主体；Cookie/Bearer 主体冲突拒绝；system=false。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `bearer` | `string / null` | 见引用类型的完整约束 |
| `cookie` | `string / null` | 见引用类型的完整约束 |
| `peer_address` | `string` | 从 socket 获得，不采信代理正文 |
| `peer_workload_identity` | `string / null` | 见引用类型的完整约束 |

输出 `AuthContext` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Identity.revoke

`async fn revoke(ctx: &AuthContext, request: IdRequest) -> Result<Ack, PlatformError>`

期限上限：10000ms。事务：锁凭证并递增授权版本、写 outbox/审计，同事务。

重复撤销成功，不能恢复 revoked。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `id` | `string` | 目标资源 UUID |

输出 `Ack` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

