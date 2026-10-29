# OutboundProxy 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## OutboundProxy.invoke

`async fn invoke(ctx: &AuthContext, request: MicroProxyInput) -> Result<MicroProxyResult, PlatformError>`

期限上限：4000ms。事务：发送前事务锁Call冻结计划/seq/alias并记录；网络不持有数据库长事务；结果短事务落账。

从实际委托导出受控微Call，逐请求核查绑定/Secret/出口；固定HTTPS模板注入密钥，拒绝任意URL/headers；相同seq重放已有结果，unknown封锁Call并覆盖脚本成功结果。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `call_id` | `string` | 见引用类型的完整约束 |
| `endpoint_alias` | `string` | 见引用类型的完整约束 |
| `sequence` | `integer` | 见引用类型的完整约束 |
| `payload` | `JsonValue` | 见引用类型的完整约束 |
| `payload_digest` | `string` | 见引用类型的完整约束 |

输出 `MicroProxyResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `RESOURCE_EXHAUSTED`, `UPSTREAM_OUTCOME_UNKNOWN`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

