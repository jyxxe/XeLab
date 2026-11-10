# 内核配置文件与启动校验

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 位置与格式

宿主配置默认 `/etc/xelab/kernel.toml`，结构以 [kernel-config.schema.json](../contracts/kernel-config.schema.json) 为准，样例见 [kernel.toml](../contracts/examples/kernel.toml)。TOML 解析后用该 Schema 校验，拒绝未知字段；配置中的 secret 文件路径必须为绝对路径，内容不得打印。

| 配置块 | 必填内容 | 行为 |
|---|---|---|
| server | listen、public_origin、tls_cert_file、tls_key_file、unix_socket、host_rpc_listen | TLS 网关、本机入口及受控插件 HostService |
| database | dsn_file | 从 0600 文件读取连接参数，不放到 API 或日志 |
| runtime | socket、namespace、network_cidr | 运行时连接及专用插件网络；namespace 不替代网络/权限隔离 |
| resources | NodeConfig 全字段 | 可选 profile 默认 standard；compact 的容量与并发限制见下文 |
| storage | root_path、quota_backend、off_host_backup_uri | 专属目录、硬配额、宿主外备份目的地 |
| security | token_hmac_key_file、master_key_file、workload_ca_cert_file、workload_ca_key_file | 凭证/加密主密钥及工作负载CA私钥禁止插件挂载；仅CA公证书可共享 |
| quota_defaults | QuotaSpec | 新根的配额初始策略，下级仍受祖先累计限制 |
| ui_gateway（可选） | base_domain、public_port、shell_origins | iframe独立源映射；DNS/TLS由部署者配置，缺失仅使UI贡献unavailable |

## compact 节点

保持旧配置省略 `resources.profile` 的 standard 行为。只有显式设置 `profile = "compact"` 才采用内核 500mCPU/192MiB、系统 250mCPU/256MiB 的最低预留及有界运行参数。完整示例见 [kernel-compact.toml](../contracts/examples/kernel-compact.toml)，数据库建议见 [postgresql-compact.conf](../contracts/examples/postgresql-compact.conf)。

目标为 1 vCPU/512MiB 整机；实测有效内存至少 480MiB，插件池从实测容量扣除，最小系统不默认启用 WebUI。profile在新节点初始化时选择；与已有node_budget不一致时启动拒绝，在线更新不能切换profile。已有节点须使用显式迁移（尚未提供）或新节点，不能仅改文件再重启。安装、鉴权、审计、TLS、cgroup 和持久性门禁保持有效。详见 [compact 运行与验收](../implementation/compact-node.md)。

compact启动仍使用同一条 `xelabd --config ...` 命令：先持有宿主排他租约、保存原有效容量、设置内核cgroup限额并入组，再exec自身一次。租约跨exec保持，第二阶段在组内建立Schema/Tokio/数据库状态，复核实际成员及宿主/祖先容量；先用TOML启动限额，之后应用已持久化的同profile预算。共享文件页可能仍计于原组，稳定性验收还须采集整机内存及daemon RSS/HWM，不能只看kernel的memory.current。

## 启动顺序

1. 读取配置、文件权限、TLS、公钥/主密钥；不得因 WebUI 不在而失败。
2. 连接 PostgreSQL、检查 schema_migrations 校验摘要，迁移需显式运维动作。
3. 获取本机控制器领导锁；预算发现与宿主容量核对。
4. 验证 cgroups v2、非特权隔离、网络默认拒绝、硬磁盘配额。
5. 对账容器、卷、预留、任务和路由；对账期间 readiness=503，禁止新调度。
6. 管理接口 Ready 后按依赖及预算恢复插件，最后选择性恢复 WebUI。

宿主无法满足内核和系统预留则部署检查失败；仅插件池不足时内核可就绪，相关插件等待资源。关键配置错误不能以“警告后忽略”的方式降级绕过安全要求。

NodeConfig 的线上变更写入数据库并覆盖 resources 对应的运行预算；其他监听器/密钥路径变更需要重启。启动时检测磁盘配置和已提交预算差异，优先使用有审计的最新数据库预算，低于固定底线仍拒绝。

## iframe源映射

贡献源固定为`https://d-<deployment UUID去连字符>.<ui_gateway.base_domain>`，public_port非443时追加端口；路径由PluginUiSpec.base_path与entry_path组成。主机名映射必须精确命中已批准部署，未知Host或伪造转发头拒绝，不能根据请求Host任意构造API返回URL。

base_domain是受控DNS后缀，逐label验证，不接受IP、通配符或空label。它是专用UI来源命名空间，server.public_origin的可信Core主机不能等于该域或位于其子域；升级前若原配置重叠，应配置独立DNS/TLS名称并保留CLI恢复入口。壳的frame-src可固定为该受控HTTPS命名空间，不从全局部署目录枚举私有UUID，Host路由和逐次审批仍须精确验证。

shell_origins为显式允许公布并嵌入的精确外部HTTPS源，不能与贡献源或内核认证源相同。列表可以为空：裸公开贡献只加入当前可用、原本由根路径公开的默认WebUI。其他平台生成的WebUI须经可信发布者、最新健康单实例、当前部署审批和active Client精确注册核验，并使用[贡献规范](../plugins/ui-contributions.md)的framing-only政策证明，才把一个精确动态父源加入frame-ancestors；不公布所有已部署壳源，不使用frame-ancestors通配。普通Client的任意origin不会加入。此机制不取代OAuth/用户授权，也不自动放开API CORS。未知父源和与贡献同源的父页面继续拒绝。

部署者将通配DNS/TLS和外部入口指向平台受控静态网关，可由同节点反向代理完成；网关只能访问批准的容器HTTP8080。DNS/证书未完成时贡献显示unavailable，不能降级到同源frame；内核管理API和CLI继续可用。修改源配置需重启并使旧frame通道失效。
