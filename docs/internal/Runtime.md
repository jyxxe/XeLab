# Runtime 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Runtime.ensure

`async fn ensure(ctx: &AuthContext, request: EnsureInstanceInput) -> Result<RuntimeObservation, PlatformError>`

期限上限：60000ms。事务：数据库计划已提交；外部 OCI 操作不在长事务内。

按 instance_id 标签幂等，镜像/代次/资源冲突隔离，不创建第二个实例。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `instance_id` | `string` | 稳定实例 UUID |
| `deployment_id` | `string` | 部署 |
| `generation` | `integer` | 代次 |
| `installation_id` | `string` | 已验证安装 |
| `reservation_id` | `string` | 已提交预留 |
| `volume_ids` | `array` | 挂载对象 |
| `config_digest` | `string` | 普通配置 SHA-256 |

输出 `RuntimeObservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.observe

`async fn observe(ctx: &AuthContext, request: IdRequest) -> Result<RuntimeObservation, PlatformError>`

期限上限：2000ms。事务：真实运行时查询，不把期望状态当观测。

不存在返回 exists=false；运行时不可达返回 PROVIDER_UNAVAILABLE。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `id` | `string` | 目标资源 UUID |

输出 `RuntimeObservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.stop

`async fn stop(ctx: &AuthContext, request: StopInput) -> Result<RuntimeObservation, PlatformError>`

期限上限：302000ms。事务：外部停止后观测，再由控制器提交结果。

撤路由/Drain 后执行，停止和删除可重放，不删除持久卷。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `instance_id` | `string` | 实例 |
| `grace_ms` | `integer` | 见引用类型的完整约束 |

输出 `RuntimeObservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

