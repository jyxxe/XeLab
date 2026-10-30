# 单节点资源预算与准入

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 计算口径

CPU 使用 millicpu，1000 表示 1 vCPU 时间预算。内存、临时盘、持久盘统一使用 bytes，HTTP 整数不超过 2^53-1。最低内核预留不是整机最低配置，也不是内核实测常驻使用量。

```text
plugin_cpu_pool = host_cpu - kernel_reserved_cpu - system_reserved_cpu
plugin_memory_pool = host_memory - kernel_reserved_memory - system_reserved_memory
kernel_reserved_cpu >= 4000 millicpu
kernel_reserved_memory >= 17179869184 bytes
sum(active_instance_cpu_limits) <= plugin_cpu_pool
sum(active_instance_memory_limits) <= plugin_memory_pool
```

内核池包含核心网关、控制器、授权、任务控制、管理 API；宿主池包含 OS、containerd、独立 PostgreSQL 与日志后端；WebUI 和所有业务实例进入插件池。每个进程必须明确归属，避免漏算或重复计入。

## 准入步骤

锁定节点预算行→读取当前宿主可分配容量→校验最低需求和批准上限→计算新旧版本增量→验证卷/临时盘硬配额→持久化 Reservation 和 Instance→提交后创建容器。

资源不足不创建额外实例；异步操作进入 waiting_resources 并保留 Operation 原因，同步预检返回 RESOURCE_EXHAUSTED。共享依赖实例只计一次，消费者数量不复制预留。

节点配置更新以 NodeStatus ETag 为前置条件，不能降低内核最低预留，也不能把插件池缩到已使用预留以下。超额时返回 409，管理员先停用或缩减实例。

## 强制实施

插件父 cgroup 设置合计 CPU/内存上限，实例设置各自限额；内核资源组实施内存保护。CPU weight 不等于保留四核；物理核独占需另行 cpuset 配置。系统未纳管负载必须计入 system_reserved，否则预算保证前提不成立。

内核自身超出预留的增长需另行提高预留/余量；规格不承诺无限负载下控制面始终可用。实现控制语义以 [Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) 为依据。

## 微部署的父池内资源

Micro Plugin Runtime父实例仍按普通OCI预算计一次，默认4000mCPU/4GiB。微部署无需单独OCI；resources默认250mCPU/268435456字节（256M按256MiB），是该部署所有活动worker的合计限额，资源不能按Call或子账户复制。

核心受审适配器管理父MPR下control与workloads资源树及每微部署叶子cgroup。默认control_cpu_reserve_millis=500、maximum_sandbox_cpu_millis=3500；control_memory_reserve_bytes=128MiB、制品缓存256MiB、maximum_sandbox_bytes=3758096384（3584MiB）。Initialize同时验证CPU500+3500≤4000m和内存128+256+3584=3968MiB<4GiB，否则拒绝Ready。

workloads容量按活跃微部署计量：首个worker启动前原子预留该部署完整resources，组内其他worker只消耗已获准活动槽及共享组额度；最后worker退出且清理确认后释放。默认3584MiB/3500m池在默认微资源下仍以全实例4个活动槽为更严格上限，可同时准入4个不同微部署各1个Call；不把池容量推导成更多执行槽，排队也不能无限占用未激活组额度。

cpu.max控制时间带宽，250m不是独占四分之一物理核或最低性能承诺；memory.max控制cgroup计量内存，包含RSS、页缓存等，不能当作JS heap。引擎heap/stack/输入输出缓冲乘允许并发须不超过组memory；native/共享页与监督器开销仍按实际计量和hardmax管理。worker虚拟地址空间1GiB不等同RSS，不按其乘worker数预留。

微设置只能在停用、disabled且排空worker/Prepare/queued后按ETag调整，更新revision/generation并使旧句柄失效；settings不改变微制品/原limits/父绑定/候选Skill。提高资源须重新通过父workloads准入，不扩大节点插件池或借内核至少4C16G。详见[MPR配置](../preset-plugins/micro-plugin-runtime/configuration.md)。

## 扩展的计账归属

Extension Plugin不增加独立OCI预留。data贡献的缓存、解析和执行按实际根服务预算计量；根为微部署时继续使用该部署聚合250mCPU/256MiB、MPR父默认4C/4GiB及原heap/stack限制，不因扩展层数复制额度。扩展可配置的limits只能收紧实际根及扩展点的既有预算。

skill模式的handler若在另一个服务执行，按该服务现有实例/微组准入及调用槽计账；根调用链共享原账户树计数，不把每个扩展层当作新根额度。所有缓存及在途上下文有界，链深最多16层，不能借扩展无限增长内核或宿主内存。
