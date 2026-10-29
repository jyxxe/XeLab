# DependencyResolver 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## DependencyResolver.plan

`async fn plan(ctx: &AuthContext, request: DependencyPlanRequest) -> Result<DependencyPlan, PlatformError>`

期限上限：10000ms。事务：读取版本一致的安装/部署/审批快照。

无外部副作用；强依赖环拒绝，执行前重新核对计划 revision。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `deployment_id` | `string` | 资源唯一标识 |
| `installation_id` | `string` | 资源唯一标识 |

输出 `DependencyPlan` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

