# 单节点资源预算与准入

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

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
