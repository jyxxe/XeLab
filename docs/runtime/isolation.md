# 容器隔离与网络策略

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

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

[Micro Plugin Runtime](../preset-plugins/micro-plugin-runtime/README.md)本身仍遵守本页OCI边界；其微插件无需独立OCI，但必须在受限VM/受控worker中执行。普通同UID的Python/Node子进程不具备该隔离能力，禁止默认授予文件/网络/HostService。逐微堆/stack/中断/墙钟限制与父cgroup总CPU/RSS边界不同，详见[执行隔离](../preset-plugins/micro-plugin-runtime/security.md)。
