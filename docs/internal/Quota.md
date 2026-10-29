# Quota 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Quota.reserve

`async fn reserve(ctx: &AuthContext, request: QuotaReserveInput) -> Result<Reservation, PlatformError>`

期限上限：10000ms。事务：按根至叶锁全部 quota 行，增量占额与预留同事务。

operation_id 唯一防重复计数，检查所有祖先；调用每日额度同事务累计。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `account_id` | `string` | 目标账户 |
| `kind` | `['descendant', 'api_key', 'call']` | 见引用类型的完整约束 |
| `amount` | `integer` | 正数 |
| `operation_id` | `string` | 关联操作/调用，防止重复占额 |

输出 `Reservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Quota.release

`async fn release(ctx: &AuthContext, request: IdRequest) -> Result<Ack, PlatformError>`

期限上限：10000ms。事务：锁预留行；首次 released 才减并发/对象计数。

重复释放不重复减计数；已消费日调用量不回滚。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `id` | `string` | 目标资源 UUID |

输出 `Ack` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

