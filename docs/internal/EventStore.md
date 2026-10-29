# EventStore 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## EventStore.append

`async fn append(ctx: &AuthContext, request: EventPublish) -> Result<Event, PlatformError>`

期限上限：10000ms。事务：去重指纹、事件/outbox 与业务变更同事务。

topic/version 授权有效；同键不同 payload 冲突。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `topic` | `string` | 主题命名空间 |
| `version` | `string` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `payload` | `JsonValue` | 见引用类型的完整约束 |
| `dedupe_key` | `string` | 发布去重键，最长保留 24h |

输出 `Event` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## EventStore.lease

`async fn lease(ctx: &AuthContext, request: LeaseEventInput) -> Result<LeaseEventResult, PlatformError>`

期限上限：10000ms。事务：锁 subscription，最多一个 leased delivery。

无事件返回 null；确认前不推进游标，30s 到期重投。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `subscription_id` | `string` | 订阅 |
| `lease_seconds` | `integer` | 见引用类型的完整约束 |

输出 `LeaseEventResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## EventStore.ack

`async fn ack(ctx: &AuthContext, request: AckDelivery) -> Result<Ack, PlatformError>`

期限上限：10000ms。事务：校验当前 lease_token，更新游标或重试状态。

旧租约不能确认新投递；重复合法 ack 成功。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `delivery_id` | `string` | 资源唯一标识 |
| `lease_token` | `string` | 投递租约凭证 |
| `outcome` | `string` | ack 推进游标，retry 退避，dead_letter 停止本消息 |

输出 `Ack` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

