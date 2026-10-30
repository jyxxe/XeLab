# ExtensionExecutor 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## ExtensionExecutor.invoke

`async fn invoke(ctx: &AuthContext, request: SkillInvokeInput) -> Result<SkillCallResult, PlatformError>`

期限上限：30000ms。事务：短事务固定Skill/扩展目标/身份闭包及每Call动态执行链、委托和副作用分类；外部宿主/handler执行不持有长数据库事务。

验证固定extension Skill目标与核心发布来源；联合skill.invoke、extension.invoke、每层真实宿主extension.host.use、原root访问动作及handler当前Skill权限，始终使用原subject/来源链。冻结ExtensionHostSnapshot，每层当前generation和root实例代次均须可路由；每次出站及嵌套handler执行再验撤权、DAG与调用祖先无重入。调用ExtensionHostService.Invoke到实际物理root，data由point适配器解释已发布JSON；skill由监督器调用固定handler，不执行即时源码/任意URL/宿主模块。输入/输出与point/handler/root最小限制一致，micro数据点维持MicroManifest原Schema及side_effect，沿已有资源组/Attach和联网开关准入，默认250m/256MiB是宿主微部署聚合预算而非子扩展新预留。副作用/取消/unknown规则不得降级，复用核心Call/Capability Cancel/QueryResult及异步batch；后代或handler结果来源与当前Call权限隔离。 ordinary root按扩展点专用host.use与当前审批准入，不要求无关capability.invoke；micro root另查micro.invoke/runtime.use。只检查leaf extension.invoke，祖先仅提供宿主点时不隐式执行其全部Skill；实际执行祖先handler才另查其invoke/Skill授权。 micro根先从真实闭包读取根微对象并持久化Call.micro_*执行快照，经Runtime.configure_micro_group取得本ExtensionCall资源句柄，填ExtensionInvokeRequest.resource_group_handle；MPR从已Ready的微prepared registry读取原代码/point/limits，并由AttachMicroWorker确认入组后执行。不得从contribution自报源码/微版本，不复制调用MicroRuntimeService.Invoke；ordinary根handle为空。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `skill_id` | `string` | UUID |
| `request` | `SkillInvokeRequest` | 见引用类型的完整约束 |

输出 `SkillCallResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `DEPENDENCY_CONFLICT`, `CONTRACT_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

