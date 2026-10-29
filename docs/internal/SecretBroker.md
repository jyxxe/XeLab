# SecretBroker 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## SecretBroker.lease

`async fn lease(ctx: &AuthContext, request: SecretLeaseInput) -> Result<SecretLeaseResult, PlatformError>`

期限上限：2000ms。事务：验证实际工作负载、绑定、当前 secret 状态及版本。

返回值只存在安全通道内存，不序列化到普通审计和幂等缓存。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `binding_id` | `string` | 绑定对象 |
| `requested_seconds` | `integer` | 见引用类型的完整约束 |

输出 `SecretLeaseResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

