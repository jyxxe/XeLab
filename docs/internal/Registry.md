# Registry 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Registry.publish

`async fn publish(ctx: &AuthContext, request: ProviderPublishInput) -> Result<RegistryRevision, PlatformError>`

期限上限：10000ms。事务：校验代次、审批、健康后提交 provider_records。

端点仅内核可见；未 Ready 能力不进入可用快照。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `provider` | `CapabilityProvider` | 见引用类型的完整约束 |
| `generation` | `integer` | 观测代次 |
| `rpc_endpoint` | `string` | 已由运行时验证的端点 |

输出 `RegistryRevision` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Registry.withdraw

`async fn withdraw(ctx: &AuthContext, request: IdRequest) -> Result<RegistryRevision, PlatformError>`

期限上限：2000ms。事务：撤销指定 instance_id 的所有可用能力记录。

快照原子替换；重复撤销成功，不删除部署。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `id` | `string` | 目标资源 UUID |

输出 `RegistryRevision` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Registry.resolve

`async fn resolve(ctx: &AuthContext, request: ResolveInput) -> Result<Binding, PlatformError>`

期限上限：2000ms。事务：读当前注册表并校验当前授权；固定绑定落库。

先权限/契约/特性过滤再确定性选择；不返回裸端点给插件。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `contract` | `ContractRef` | 见引用类型的完整约束 |
| `binding_scope` | `['request', 'deployment', 'task']` | 见引用类型的完整约束 |
| `scope_id` | `string` | 调用/部署/任务 ID |
| `preferred_deployment_id` | `string / null` | 见引用类型的完整约束 |
| `required_plugin_id` | `string / null` | 见引用类型的完整约束 |
| `required_installation_id` | `string / null` | Skill调用必填为签名来源安装；直接调用可为空 |

输出 `Binding` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

