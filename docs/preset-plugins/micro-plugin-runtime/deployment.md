# MPR部署、故障与恢复

[MPR目录](README.md) · [配置](configuration.md) · [核心部署](../../operations/deployment.md)

> 规格版本：2.8.0；状态：待实现。资源建议尚未通过并发、内存峰值或故障恢复验收。

## 一份Runtime，多份微插件

MPR以`org.xelab.micro-plugin-runtime@1.3.0`普通OCI插件安装和部署。默认单副本、无持久卷，data_schema_version=none；镜像、引擎构建和安装摘要仍受平台制品审核与生命周期约束。

默认4000mCPU/4GiB内存/256MiB临时盘/pids128，活动4、队列32/5s，均从插件池扣除。宿主基础设施另行预留，不能为运行更多微插件降低内核至少4C16G预留。

微版本及代码由核心持久保存，微部署绑定明确Runtime安装/部署。微插件不需要镜像构建、独立容器、宿主端口或数据卷；它使用的是Runtime内部有限执行单元，不是另一个平台容器。

## 启动与就绪

MPR就绪前，核心受审OCI适配器建立父实例control/workloads资源树并验证cpu/memory控制器、进程身份/入组、group kill和回收；MPR再验证受限JS、虚拟I/O及可强制终止worker。任一能力缺失拒绝Ready，不通过共享进程VM或普通解释器子进程降级冒充硬限制。

监督器创建尚未加载用户代码且停等的短命worker，携当前微Call委托、核心group不透明句柄、容器PID及启动时间调用HostService.AttachMicroWorker。核心按namespace/start-time/pidfd核实身份并迁入正确微部署子组，成功Ack后才允许加载JS；失败由监督器/核心终止回收，启动占用按control预算有界计量。

核心只在当前Runtime安装/部署已批准且实例Ready后为绑定微部署准备制品。Prepare发生在有界资源下；缓存缺失时准备原固定摘要，不自动从名称、最新版本或外部URL取替代代码。

MPR就绪不是所有微插件自动启用，也不自动批准代码。发布微版本不生成候选；新建MicroDeployment才生成candidate_skill，初始disabled且不自动register/enable。具体微部署、注册后的Skill和来源链均须当前有效；核心本身和其他OCI插件不等待MPR才能启动。

微部署默认resources为250mCPU/256MiB、network_enabled=false。Prepare/Ready可以离线完成；pure正常调用，HTTP profile关闭网络时在创建worker前明确PERMISSION_DENIED，不消费HTTP序号或启动服务商fallback。

## 排空与关闭

微部署停用先原子enabled=false/generation++撤新路由并进入draining，取消队列/Prepare/worker与句柄，组清理完成才disabled；失败保持draining/failed，禁止settings。父Runtime的Drain同样只等待有限时间，到期终止对应执行单元，Shutdown结束worker并释放缓存，不无限等待脚本退出。

普通单worker崩溃/取消只影响其调用；微部署子cgroup OOM按组终止该部署相关worker。核心收集退出/OOM证据，最后worker退出且空组/清理确认后释放该活跃微部署的资源；HTTP已发结果不明仍保留unknown。MPR整体退出可能影响全部微调用，相关微部署进入waiting_runtime，不把共享父实例视作独立故障域。

## 重启、升级与缓存

Runtime重启、升级或恢复后缓存为空。核心从自己的不可变微制品记录重新准备，不能依赖旧进程内存或临时盘恢复代码身份。LRU淘汰只释放缓存，不归档微版本或撤销核心已保存结果。

更换Runtime安装需显式发布固定新安装的微版本，再创建新MicroDeployment和Skill；修改父Runtime部署或原limits也须新建微部署。resources/network_enabled可在停用排空后用settings修改，推进revision/generation、使旧句柄失效；不改原候选Skill或父绑定。waiting_runtime不授权自动换安装，旧代次的迟到消息不得覆盖新状态。

核心保存Call状态与成功结果；无持久卷的MPR不承诺恢复执行内存。pure未确认结果按固定版本安全恢复；HTTP profile按non_repeatable及持久代理台账核实，不自动重跑，不能误报成功或悄悄换新版代码。

## 容量管理

制品缓存默认256MiB、最多128份、TTL300s；control另保留128MiB/500mCPU，workloads池为3584MiB/3500mCPU。Initialize验证内存合计3968MiB<父4GiB、CPU合计4000m≤父4C。每worker地址空间保护为1GiB，仅计虚拟地址空间，不能按其乘以worker数预留RSS。

每个活跃微部署只计一次resources，组内所有worker共享其CPU/内存；最后worker确认退出/清理后释放。默认3584MiB/3500m池在默认微资源下仍受实例4槽约束，最多4个不同微部署各1个Call同时活跃；不把资源池容量当作放宽并发上限。单微部署默认并发1、硬4，再与实例槽、组内存和账户额度取交集；更多微部署不会复制池。

设置变更流程为停用→等待phase=disabled及无worker/Prepare/queued→携ETag PUT settings→重新启用。修改资源时检查其不超过父池，并验证heap/stack/输入输出缓冲预算乘允许并发不超过组memory；native开销仍受hardmax及实测约束，不能仅靠预算公式声称已测稳定。

默认wall2s、heap16MiB并不证明4个最坏输入可同时稳定运行。发布前需以最大输入、最大输出、编译开销、日志洪泛、取消和引擎异常测量峰值，按结果调整建议资源与验收范围。
