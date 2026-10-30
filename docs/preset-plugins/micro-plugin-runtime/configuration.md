# MPR配置与资源限额

[MPR目录](README.md) · [执行协议](execution.md) · [节点预算](../../runtime/budgets.md)

> 规格版本：2.8.0；状态：待实现的配置契约。机器字段以预设清单、默认配置和核心模型为准。

## 实例预算

MPR默认4000mCPU、4294967296字节内存、268435456字节临时盘、pids128，单副本。活动请求上限4，队列上限32，排队上限5000ms；控制RPC使用独立有界处理通道，不排到脚本业务队列尾。

MicroDeployment.resources默认cpu_millis=250、memory_bytes=268435456；这里用户的256M按256MiB解释，0.25vCPU按250mCPU时间带宽解释。资源是该微部署所有活动worker的合计上限，不是每Call各复制一份；单微部署默认活动并发1、硬上限4，同组worker共享该上限。

所有worker、VM、制品缓存、缓冲区和监督器都计入MPR父cgroup。每微部署子cgroup实施真实CPU带宽与计量内存限制；memory_bytes涵盖内核计量的RSS、页缓存等，不等同JS heap或单一RSS计数。引擎heap/stack限制继续执行，不因增大resources而自动放宽。

父MPR下由核心受审OCI适配器建立control和workloads资源树，所有JS位于独立短命worker进程。control预算为制品缓存加128MiB控制内存、500mCPU；workloads总上限为maximum_sandbox_bytes=3758096384（3584MiB）、maximum_sandbox_cpu_millis=3500。每个活跃微部署位于workloads下单独子cgroup，首个worker激活时计一次资源、最后worker退出且清理确认后释放。

Initialize验证`artifact_cache_max_bytes + maximum_sandbox_bytes + control_memory_reserve_bytes <= 父memory_bytes`和`maximum_sandbox_cpu_millis + control_cpu_reserve_millis <= 父cpu_millis`。默认内存256+3584+128=3968MiB<4GiB，CPU3500+500=4000m；任一越限或缺失可靠子cgroup/worker隔离能力都拒绝Ready，不降级为共享进程VM。

manifest.minimum_resources保留1vCPU/1GiB作为调整配置后的启动下限，不是默认部署资源。默认模板仍为4vCPU/4GiB；若手动降低父limits，必须同时缩小工作池、控制及缓存预算并重新验证，不能只改父limits而沿用4C/4GiB配置。

默认3584MiB/3500m工作池可容纳4个默认资源、各1个活动Call的不同微部署，仍受全实例4活动槽限制；扩大父池不放宽该并发上限。同一活跃微部署也可在其256MiB/250m总限额内使用多个允许槽。提高微部署资源会减少其他部署容量，不自动扩容父实例。

worker_address_space_bytes改为1GiB，仅是每个worker虚拟地址空间保护，不是RSS预留；它不复制父内存，也不替代微部署子cgroup。核心管理资源树及已验证PID，MPR没有privileged、可写宿主cgroup或运行时socket。

## 手动调整微部署设置

`PUT /api/platform/v1/micro-deployments/{id}/settings`完整提交`{resources:{cpu_millis,memory_bytes},network_enabled}`，携微部署If-Match和Idempotency-Key；具体动作和模型以核心接口定义为准。

只有enabled=false、phase=disabled，且已排空所有queued请求、Prepare和worker时可修改。同值请求是无副作用确认；其他状态返回STATE_CONFLICT，不在线修改正运行资源组。变更检查父资源池/审批上限，更新revision/generation并使旧准备句柄失效，之后重新启用/Prepare。

resources/network_enabled是独立运行设置；原limits、微版本和父Runtime绑定仍不可变。设置更新不改包/源码摘要、业务Schema或candidate_skill定义，不以更改CPU/内存为由换代码。修改原limits或父绑定仍需新建微部署。

停用先原子写enabled=false、generation递增并撤新路由，phase进入draining；队列、Prepare、worker、句柄和资源组全部取消/回收后才为disabled。清理失败保持draining/failed，禁止趁未排空提交settings。

network_enabled默认false；true只允许controlled-http-v1。开启仅允许版本已声明endpoint的受控HTTP，仍须当前micro.http.request、原微权限、HttpBinding/secret及父出口审批；不开放真实fetch/socket。Prepare/Ready可离线完成，controlled-http业务Invoke在关闭状态于创建worker前返回PERMISSION_DENIED，violations.field=network_enabled，不消耗HTTP序号或触发服务商fallback。

## 单次执行

| 限额 | 默认 | 硬边界与语义 |
|---|---|---|
| 总期限 | 2000ms | 最多30000ms；含本次Prepare、排队、编译、执行单元启动、代码求值、输入处理和结果校验 |
| JS heap | 16MiB | 最多64MiB；达到上限终止本次VM |
| JS stack | 1MiB | 当前硬上限1MiB；深递归不能耗尽监督器栈 |
| 输入JSON | 1MiB | UTF-8字节计量、深度最多64；更低的微manifest或核心限制优先 |
| 输出JSON | 1MiB | 全部stdout字节计量、深度最多64；不能截断后伪造合法JSON |
| 日志 | 64KiB | stderr独立有界，超过后截断并标记，不单因日志截断判业务失败 |
| 单微部署并发 | 1 | 硬上限4；同父MPR的多个微部署合计不能突破实例总4 |
| 微部署合计CPU | 250m | 同组所有活动worker共享0.25vCPU时间带宽，受父workloads池限制 |
| 微部署合计内存 | 256MiB | 同组所有活动worker共享268435456字节cgroup计量内存上限 |

用户传入的timeout不能延长微版本/微部署声明的总期限或根调用deadline。单次期限覆盖其Prepare、排队、编译、转发和执行；各步骤只能使用原期限剩余时间，重Prepare不能重新获得完整2000ms。

达到引擎内存、栈或执行期限时必须释放VM和活动槽。引擎中断无法覆盖的native操作需置于可强杀worker中，监督器按硬wall期限结束整个执行单元；不能只设置一个脚本可忽略的计时器。

CPU被节流的时间和HTTP等待仍计整体wall期限；250m是带宽上限而非独占物理核或最低性能保证。微部署cgroup OOM会终止该组相关worker，同组Call按实际证据失败或unknown；已经发送的HTTP不能因OOM被假称撤回。

组配置验证`max_active_requests × (heap_bytes + stack_bytes + maximum_input_bytes + maximum_output_bytes + maximum_stderr_bytes) <= resources.memory_bytes`。引擎native、编译数据和共享页还受cgroup计量/hardmax及压力实测约束，公式通过不等于保证所有最坏输入稳定运行；不按worker虚拟地址空间乘并发预留RSS。

## 制品缓存

核心持久保存微manifest、代码制品、版本和审核关系；MPR只缓存已经核验的不可变制品。默认总计量256MiB、最多128份、TTL300s，任一边界先触及即过期或按LRU淘汰。Prepare返回实际到期时间，但条目可被提前淘汰，句柄不保证驻留到期。

缓存键至少绑定package_digest、engine版本及对应Runtime安装/代次；数据、编译常量、键、索引和计量元数据都纳入总预算。每次执行创建新VM，缓存不能保留某用户的全局变量、输入输出、原始凭证或可复用委托。

缓存命中不替代当前权限、启用状态和微部署绑定检查。缓存不可用或条目被淘汰时，核心可对原固定制品重新Prepare；不允许按同名latest版本补位。超缓存容量的制品不得通过无限临时副本绕开父内存约束。

配置经现有MPR部署管理与生命周期生效。缩小缓存或实例内存时，必须在新运行代次就绪前释放旧缓存并满足新上限；不能先报告新额度已应用而继续长期超额驻留。

v1的artifact_cache_max_bytes允许1..512MiB（最低1048576字节），仍须满足父内存合计约束；不接受0。Prepare必须驻留有界制品才能Invoke，单制品超出容量时明确RESOURCE_EXHAUSTED，不无限循环重准备；动态输入/输出不计入该代码缓存。
