# MicroExecutor 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## MicroExecutor.invoke

`async fn invoke(ctx: &AuthContext, request: SkillInvokeInput) -> Result<SkillCallResult, PlatformError>`

期限上限：30000ms。事务：短事务固定微/父依赖和授权；对象复制、Prepare/执行不持有长DB事务。

skill.invoke、micro.invoke与micro.runtime.use联合检查；双层槽/预算准入，调用受控MicroRuntimeService并验证微输出Schema，持久结果附微制品来源；无任意source/path或通用执行旁路。 controlled-http-v1另检查micro.http.request并冻结绑定计划/Secret版本；non_repeatable禁止盲重放，返回前检查HTTP台账unknown/终止错误优先。 受控HTTP且network_enabled=false时在worker/HTTP sequence之前PERMISSION_DENIED，violations.field=network_enabled，不fallback；pure不受此联网开关阻断。固定micro_deployment_generation/resources/network快照，父池/微组按部署聚合准入，获得每Call资源句柄；worker必须由HostService.AttachMicroWorker确认真实入组后才执行，结束后依据pidfd回收。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `skill_id` | `string` | UUID |
| `request` | `SkillInvokeRequest` | 见引用类型的完整约束 |

输出 `SkillCallResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

