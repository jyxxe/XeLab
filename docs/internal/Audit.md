# Audit 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Audit.append

`async fn append(ctx: &AuthContext, request: AuditAppendInput) -> Result<AuditEvent, PlatformError>`

期限上限：10000ms。事务：追加，与业务事务共提交或安全失败通道持久缓冲。

调用方不得伪造 ctx；时间、ID、主体和策略版本由内核写入。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `action` | `string` | 注册动作 |
| `resource` | `ResourceSelector` | 见引用类型的完整约束 |
| `outcome` | `['allowed', 'denied', 'succeeded', 'failed']` | 见引用类型的完整约束 |
| `reason_code` | `string` | 稳定原因码 |

输出 `AuditEvent` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

