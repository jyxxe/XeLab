# MicroRegistry 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## MicroRegistry.publish

`async fn publish(ctx: &AuthContext, request: MicroPublishInput) -> Result<MicroPluginVersion, PlatformError>`

期限上限：10000ms。事务：短事务固定微/父依赖和授权；对象复制、Prepare/执行不持有长DB事务。

校验完整微资源权限、源Blob、UTF8与两个JSON Schema；固定runtime安装，源码先复制私有不可变存储，再原子提交版本/幂等/outbox，不执行代码。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `micro_plugin_id` | `string` | 见引用类型的完整约束 |
| `request` | `PublishMicroVersion` | 见引用类型的完整约束 |

输出 `MicroPluginVersion` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

