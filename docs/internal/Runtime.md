# Runtime 模块接口

[内核接口目录](README.md) · [类型契约](../contracts/internal.schema.json)

> 规格性设计；进程内稳定接口，不是额外网络服务。AuthContext 来自可信认证入口，不接受外部正文直接构造。

## Runtime.ensure

`async fn ensure(ctx: &AuthContext, request: EnsureInstanceInput) -> Result<RuntimeObservation, PlatformError>`

期限上限：60000ms。事务：数据库计划已提交；外部 OCI 操作不在长事务内。

按 instance_id 标签幂等，镜像/代次/资源冲突隔离，不创建第二个实例。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `instance_id` | `string` | 稳定实例 UUID |
| `deployment_id` | `string` | 部署 |
| `generation` | `integer` | 代次 |
| `installation_id` | `string` | 已验证安装 |
| `reservation_id` | `string` | 已提交预留 |
| `volume_ids` | `array` | 挂载对象 |
| `config_digest` | `string` | 普通配置 SHA-256 |

输出 `RuntimeObservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.observe

`async fn observe(ctx: &AuthContext, request: IdRequest) -> Result<RuntimeObservation, PlatformError>`

期限上限：2000ms。事务：真实运行时查询，不把期望状态当观测。

不存在返回 exists=false；运行时不可达返回 PROVIDER_UNAVAILABLE。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `id` | `string` | 目标资源 UUID |

输出 `RuntimeObservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.stop

`async fn stop(ctx: &AuthContext, request: StopInput) -> Result<RuntimeObservation, PlatformError>`

期限上限：302000ms。事务：外部停止后观测，再由控制器提交结果。

撤路由/Drain 后执行，停止和删除可重放，不删除持久卷。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `instance_id` | `string` | 实例 |
| `grace_ms` | `integer` | 见引用类型的完整约束 |

输出 `RuntimeObservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.ensure_micro_pool

`async fn ensure_micro_pool(ctx: &AuthContext, request: MicroPoolInput) -> Result<MicroPoolObservation, PlatformError>`

期限上限：10000ms。事务：短事务锁定池/组/租约并记计划→受控宿主cgroup/pidfd操作→短事务提交观测；外部操作不持有长DB事务。

仅核心host适配器验证MPR1.2 resource_isolation声明及真实cgroupv2可用性；建立父MPR/control与workloads子树并迁移控制进程，控制CPU500m/工作池3500m，内存总和不得超过父limits。默认工作池3584MiB、控制保留128MiB、缓存256MiB；父MPR默认4000mCPU/4GiB，三项内存合计3968MiB；CPU/memory控制器不可用则拒绝ready，不软降级，不委派cgroup写权给MPR。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `runtime_instance_id` | `string` | 核心验证的资源UUID |
| `runtime_generation` | `integer` | 固定代次 |
| `workloads_cpu_millis` | `integer` | 工作池CPU；默认3500m |
| `workloads_memory_bytes` | `integer` | 工作池内存；默认3584MiB |
| `control_cpu_millis` | `integer` | 控制端CPU；默认500m |
| `control_memory_reserve_bytes` | `integer` | 控制端内存保留；默认128MiB |
| `artifact_cache_max_bytes` | `integer` | 已批准制品缓存上限；默认256MiB |

输出 `MicroPoolObservation` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.configure_micro_group

`async fn configure_micro_group(ctx: &AuthContext, request: MicroGroupInput) -> Result<MicroResourceLease, PlatformError>`

期限上限：10000ms。事务：短事务锁定池/组/租约并记计划→受控宿主cgroup/pidfd操作→短事务提交观测；外部操作不持有长DB事务。

当前授权Call固定真实微部署设置/代次，锁池与组预算按活跃微部署预留一次，首次执行建立真实微cgroup并设cpu.max、memory.max、memory.swap.max=0；默认250m/256MiB，全部同组worker共享此硬上限。相同execution_id幂等返回原租约，不重复扣额。新Call只加同组租约，CPU/内存预留总和不超池，活动worker/待执行租约合计不超父4与微固定并发。heap+stack+输入输出缓冲声明乘允许并发不得超组memory，native开销仍受memory.max。预留不足立即释放其他已取槽，不带无限等待。 此处Call可以是micro Skill或ExtensionExecutor已验证完整闭包后的micro根ExtensionCall；后者须查可信Call.micro_*快照、原微prepared registry及leaf/每层host权限，不能仅按target.kind=micro放行或拒绝，也不能凭扩展输入冒充微根。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `runtime_instance_id` | `string` | 核心验证的资源UUID |
| `runtime_generation` | `integer` | 固定代次 |
| `micro_deployment_id` | `string` | 核心验证的资源UUID |
| `micro_deployment_generation` | `integer` | 固定代次 |
| `call_id` | `string` | 核心验证的资源UUID |
| `execution_id` | `string` | 核心生成的物理执行UUID；传输重试保持不变 |
| `resources` | `MicroResourceLimits` | 见引用类型的完整约束 |
| `max_active_requests` | `integer` | 微部署固定并发，再与父总4求交 |

输出 `MicroResourceLease` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.attach_micro_worker

`async fn attach_micro_worker(ctx: &AuthContext, request: MicroWorkerAttachInput) -> Result<MicroResourceLease, PlatformError>`

期限上限：10000ms。事务：短事务锁定池/组/租约并记计划→受控宿主cgroup/pidfd操作→短事务提交观测；外部操作不持有长DB事务。

从实际MPR实例与当前Call委托核对资源租约；将container_pid在该实例namespace内映射为真实进程，核对启动时间、父监督器来源、未执行用户代码的阻塞启动状态并取得pidfd，拒绝监督器/其它实例PID。进程迁入本微部署cgroup且复核成功才返回attached；MPR收到Ack前不能加载/运行用户代码。失败由核心kill/reap待启动worker并回收租约；句柄不是宿主路径或可转移授权。不得消耗HTTPsequence。 kill/reap尚未证实退出时记录reaping并继续占预留，不得提前释放。 此处Call可以是micro Skill或ExtensionExecutor已验证完整闭包后的micro根ExtensionCall；后者须查可信Call.micro_*快照、原微prepared registry及leaf/每层host权限，不能仅按target.kind=micro放行或拒绝，也不能凭扩展输入冒充微根。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `runtime_instance_id` | `string` | 核心验证的资源UUID |
| `runtime_generation` | `integer` | 固定代次 |
| `micro_deployment_id` | `string` | 核心验证的资源UUID |
| `micro_deployment_generation` | `integer` | 固定代次 |
| `call_id` | `string` | 核心验证的资源UUID |
| `resource_group_handle` | `string` | 本Call资源准入句柄 |
| `container_pid` | `integer` | 实例PID namespace内PID，拒绝0/监督器/其他实例 |
| `process_start_time_ticks` | `integer` | 待验证启动时间；核心取pidfd防PID复用 |

输出 `MicroResourceLease` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

## Runtime.release_micro_group

`async fn release_micro_group(ctx: &AuthContext, request: MicroGroupReleaseInput) -> Result<MicroGroupReleaseResult, PlatformError>`

期限上限：10000ms。事务：短事务锁定池/组/租约并记计划→受控宿主cgroup/pidfd操作→短事务提交观测；外部操作不持有长DB事务。

核心通过pidfd退出和cgroup.events populated=0核实，不信任MPR自报退出。释放本Call租约；同组还有reserved/attached/reaping租约时保留组及单份预算。最后租约/worker退出后删除空组并释放CPU/内存预留，旧代句柄不得影响新代。实例退出由对账核实并清理；回收未完成不得提前宣称disabled或允许settings修改。

| 输入字段 | 类型 | 语义 |
|---|---|---|
| `runtime_instance_id` | `string` | 核心验证的资源UUID |
| `runtime_generation` | `integer` | 固定代次 |
| `micro_deployment_id` | `string` | 核心验证的资源UUID |
| `micro_deployment_generation` | `integer` | 固定代次 |
| `resource_group_handle` | `string` | 准确定位旧Call租约，不影响新代次资源组 |

输出 `MicroGroupReleaseResult` 及嵌套结构见 [内部Schema](../contracts/internal.schema.json) 或 [HTTP模型](../api/models/README.md)。必填字段按各自 Schema.required 判断，nullable 必须显式传 null。

错误：`INVALID_ARGUMENT`, `PERMISSION_DENIED`, `STATE_CONFLICT`, `PROVIDER_UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `DEADLINE_EXCEEDED`, `INTERNAL`。按 [错误目录](../communication/errors.md) 映射；不得从错误文本猜测是否已执行。

