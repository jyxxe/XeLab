# ExtensionController 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## ExtensionController.reconcile

`async fn reconcile(ctx: &AuthContext, request: ExtensionReconcileInput) -> Result<ExtensionDeployment, PlatformError>`

期限上限：10000ms。事务：锁全局依赖图revision/固定身份闭包并写期望状态→不持长DB锁执行Prepare/Release→CAS实际宿主/root代次及部署revision提交观测。

读取ExtensionDeployment固定版本、package、typed host、point、config、root安装与host_chain_digest；extension→ordinary/micro/extension宿主边、handler目标、普通强依赖与micro→MPR边在同一锁图事务复核DAG/最大深度16。宿主不会反向强依赖其已附加扩展，基础Ready独立。逐层检查extension.host.use及实际root兼容声明，按拓扑顺序Prepare于最终物理root OCI；host_chain_digest只含不可变身份，generation/根实例仅进运行快照，root重启不更改候选Skill定义。初始disabled，不自动注册或启用候选；enabled后父链未Ready则waiting_host，不换host/版本。停用/归档先generation++撤新路由，后代逆拓扑Cancel/Release；句柄和worker/handler实际排空后才disabled/archived，失败持续待清理。data与skill均有固定candidate_skill；原root资源、网络、micro profile与Schema限制不可被扩展放宽。 身份闭包按ExtensionIdentityClosure做JCS SHA-256，逻辑链终止于首个非extension宿主，物理micro→MPR单列root并参与DAG；每边config摘要来自对应不可变扩展部署，不把可变资源/启停纳入。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `extension_deployment_id` | `string` | 核心读取并核验的UUID |
| `expected_revision` | `integer` | 需CAS匹配的部署revision |

输出 `ExtensionDeployment` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `DEPENDENCY_CONFLICT`, `CONTRACT_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

