# MicroController 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## MicroController.reconcile

`async fn reconcile(ctx: &AuthContext, request: MicroReconcileInput) -> Result<MicroDeployment, PlatformError>`

期限上限：10000ms。事务：短事务固定微/父依赖和授权；对象复制、Prepare/执行不持有长DB事务。

固定runtime安装/部署、单副本、微generation和批准预算；Prepare后CAS绑定实际实例代次，迟到结果不覆盖，停用先撤路由/Cancel再Release，不创建独立OCI。 设置只允许enabled=false、phase=disabled且worker/队列/Prepare/资源租约已全部排空时修改；generation递增使旧Prepare失效，候选Skill不变。启用前验证resources与network设置、真实资源隔离声明；Prepare/Ready不要求联网打开。 停用立即enabled=false/generation++并进入draining，先取消queued/活动执行，再Release准备句柄；无worker/queued/Prepare/资源租约且真实回收后才disabled，控制RPC超时不能单独当作排空证明。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `micro_deployment_id` | `string` | 见引用类型的完整约束 |
| `expected_revision` | `integer` | 见引用类型的完整约束 |

输出 `MicroDeployment` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

