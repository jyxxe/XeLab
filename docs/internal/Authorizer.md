# Authorizer 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Authorizer.evaluate

`async fn evaluate(ctx: &AuthContext, request: AuthorizeInput) -> Result<AuthorizationDecision, PlatformError>`

期限上限：2000ms。事务：只读真实资源、当前角色/组/祖先/源凭证版本。

无法确认当前状态即拒绝，内部任务入口与 HTTP 共用此决策。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `action` | `string` | 注册动作 |
| `resource` | `ResourceSelector` | 见引用类型的完整约束 |
| `expected_authz_revision` | `integer` | 调用者观察的版本；0 表示首次读取 |

输出 `AuthorizationDecision` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Authorizer.invalidate

`async fn invalidate(ctx: &AuthContext, request: InvalidateInput) -> Result<Ack, PlatformError>`

期限上限：2000ms。事务：仅接收已提交版本，更新本机缓存水位。

通知可重放；不能把丢通知视为旧授权永久有效。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `subject_ids` | `array` | 失效对象 |
| `new_revision` | `integer` | 已提交的新版本 |

输出 `Ack` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

