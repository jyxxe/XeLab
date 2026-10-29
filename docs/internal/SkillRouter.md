# SkillRouter 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## SkillRouter.resolve

`async fn resolve(ctx: &AuthContext, request: SkillResolveInput) -> Result<SkillResolution, PlatformError>`

期限上限：10000ms。事务：短事务锁定 Skill 版本/审批/调用记录；外部请求不持有长数据库事务。

读取不可变定义，验证双层授权及输入 Schema，提取精确 Blob 引用；按固定 plugin_id/contract 解析，不做任意 URL 或静默换插件。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `skill_id` | `string` | UUID |
| `input` | `JsonValue` | 见引用类型的完整约束 |

输出 `SkillResolution` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## SkillRouter.invoke

`async fn invoke(ctx: &AuthContext, request: SkillInvokeInput) -> Result<SkillCallResult, PlatformError>`

期限上限：300000ms。事务：短事务锁定 Skill 版本/审批/调用记录；外部请求不持有长数据库事务。

记录 Skill 摘要和 Call，实例级准入后通过 Broker 转发；取消、期限、流控和当前来源链持续生效。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `skill_id` | `string` | UUID |
| `request` | `SkillInvokeRequest` | 见引用类型的完整约束 |

输出 `SkillCallResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

