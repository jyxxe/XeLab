# Admission 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Admission.reserve

`async fn reserve(ctx: &AuthContext, request: AdmissionInput) -> Result<Reservation, PlatformError>`

期限上限：10000ms。事务：锁 node_budget，校验代次和实例唯一预留后提交。

不能超插件池或借用内核预留，实例总量不超已批 limits。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `instance_id` | `string` | 稳定实例 UUID |
| `deployment_id` | `string` | 部署 |
| `generation` | `integer` | 代次 |
| `limits` | `ResourceLimits` | 见引用类型的完整约束 |

输出 `Reservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Admission.release

`async fn release(ctx: &AuthContext, request: IdRequest) -> Result<Ack, PlatformError>`

期限上限：10000ms。事务：确认真实停止/卸载后锁实例和预算释放。

未确认容器停止不能提前归还运行资源；可重放。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `id` | `string` | 目标资源 UUID |

输出 `Ack` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

