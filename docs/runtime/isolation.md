# 容器隔离与网络策略

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 容器创建硬约束

- 非 root UID/GID，禁止 privileged、hostPID、hostIPC、hostNetwork。
- 只读 rootfs，写入只允许已批准专属卷和有上限 tmpfs/临时盘。
- drop 全部 Linux capabilities；确需增加按平台固定允许集单独批准，禁止 SYS_ADMIN。
- no_new_privileges、平台 seccomp 配置、进程数与文件描述符限制。
- 不挂载宿主根目录、/proc 宿主视图、运行时 socket 或内核密钥文件。
- 非必需设备不暴露，当前禁止 GPU 设备和嵌套容器管理。

## 本机网络

插件网络默认拒绝直连其他插件及宿主敏感端口。内核 Broker 代理所有能力调用；HostService 是插件唯一宿主能力入口。安装不发布公网端口，网关按批准路由转发。

运行时工作负载身份绑定 instance_id/deployment_id/generation，过期、重启代次或审批撤销后须重新校验。首版使用平台签发短期 mTLS 证书或保护 UDS peer 的等价实现，凭证仅挂入对应实例只读目录。

## 出口

PluginApproval.egress 为精确协议/主机/端口白名单。经受控出口代理或网络策略执行；DNS 初次与每次连接都校验解析地址，防止重绑定访问 loopback、link-local、宿主/其他插件内网。重定向逐跳重新校验，默认最多 3 跳。

连接数、带宽、DNS、请求体和响应体均有部署级上限；不能只检查域名字符串。外部 API 密钥只交给对应绑定工作负载。

## 数据隔离边界

若插件已获得多个用户原始卷的读取权，平台逐请求授权不能限制恶意插件私下读取。共享实例优先无状态，通过 Blob/受控宿主数据接口取得本次授权数据；敏感有状态工作负载按用户/工作空间隔离实例和卷。

containerd 不负责完整网络、外部卷管理和日志持久化，平台适配器必须集成这些能力。[containerd scope](https://containerd.io/scope/)

## 微脚本计算沙箱

[Micro Plugin Runtime](../preset-plugins/micro-plugin-runtime/README.md)本身仍遵守OCI边界；微插件无需独立OCI，每次调用必须在短命worker内运行受限VM。普通同UID Python/Node子进程不具备该隔离能力；脚本不获文件、真实网络或直接HostService。网络策略默认关闭，手动开启也只允许已声明受控HTTP与当前权限/出口审批。

核心受审OCI适配器拥有cgroup管理职责，MPR没有privileged或宿主cgroup写权限。父MPR下设control叶子（监督器/缓存）与无进程的workloads内部域，后者每个活跃微部署各有叶子组；父MPR内部域也不驻留进程，以满足cgroup v2域控制拓扑。每组resources限制该微部署所有活动worker合计CPU时间带宽和计量内存，不是每Call重复配额。

监督器创建尚未装载用户代码的停等worker后，经HostService.AttachMicroWorker提交当前Call委托、核心group句柄、容器PID及启动时间；核心验证namespace、实际PID/启动身份、父实例代次及pidfd，再迁入对应组。Ack前不得执行JS，失败应终止回收；仅凭请求给出的PID或路径不能控制宿主其他进程。

核心必须验证cpu/memory控制器、先入组再执行、按组终止与空组回收能力，缺失则阻止Ready。worker地址空间上限1GiB只作虚拟空间保护，引擎heap/stack另限；微部署memory.max涵盖RSS等被计量内存。组OOM会影响该微部署的相关worker，单Call取消按稳定进程身份终止一个worker，已发HTTP仍按unknown证据处理。

最后worker退出且清理确认后才释放活跃组预留；尚有残余进程或计量内存时继续计父池。详见[MPR执行隔离](../preset-plugins/micro-plugin-runtime/security.md)和[Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html)。

## 扩展执行隔离

扩展功能仅由宿主显式版本化extension_point接纳。data贡献是≤256KiB严格JSON，不含任意可执行源码；skill模式只固定已注册处理器，由核心重新认证授权后调用，不提供动态库、任意JS/native代码、shell或宿主路径加载。

微宿主的扩展Data在MPR1.3仅以只读extensionData暴露，不能改变原输入/输出Schema、副作用、网络profile或资源组。skill请求由监督器调用核心HostService.InvokeSkill，VM没有HostService、运行时凭证或任意目标权限。扩展链共享实际根故障域；独立handler仍处于自己的原隔离域，不能借链切换成宿主服务账户。
