# M4 实现与验收记录

[实施索引](README.md) · [目标 #51](https://github.com/jyxxe/XeLab/issues/51) · [生产验收 #50](https://github.com/jyxxe/XeLab/issues/50)

本轮从 main `c861fd80a8e81d626d749e2ce9db435685f8669a`、Schema16 开始，
按依赖、HostService、事件三个阶段追加 Schema17–19。历史 SQL 和迁移校验和保持。
功能代码、专项测试、PR 完整 CI 与最终 main CI 分别核验；
最终合并提交、完整 CI 链接和完成判定以目标 #51 的收尾记录为准。
本页记录实现与可复现的验收入口；旧候选或组件测试不能计作最终主线通过。

## 实现范围

| 阶段 | 功能 | 交付与说明 |
|---|---|---|
| 依赖 | 确定性候选过滤、精确版本/安装/契约摘要/配置/代次/特性绑定；原子强依赖图和16层边界；部署及请求作用域；失败锁存、可选降级、固定升级计划 | [#52](https://github.com/jyxxe/XeLab/issues/52)、[PR #55](https://github.com/jyxxe/XeLab/pull/55)、[实现](m4-dependencies.md) |
| HostService | 实际 mTLS 实例和原 Call 委托；能力解析/调用/流，Skill 发现/固定解析/检查/调用/流；独立签名/审批上限与原来源链交集 | [#53](https://github.com/jyxxe/XeLab/issues/53)、[PR #56](https://github.com/jyxxe/XeLab/pull/56)、[实现](m4-host.md) |
| 事件 | 签名主题、发布和 outbox 同事务、24小时去重、持久游标、30秒单一租约、旧租约 fencing、重投退避/死信、保留和容量边界、公开 SSE 与 Host RPC | [#54](https://github.com/jyxxe/XeLab/issues/54)、[实现](m4-events.md) |
| 恢复 | 原生 PostgreSQL COPY 保留事件/outbox/死信及精确数值；验证源数据与恢复布局；禁用旧订阅并围栏旧租约、旧 Host ticket 和请求句柄 | [离线恢复](m2-platform-restores.md)、[事件](m4-events.md) |

插件目标的 features 使用一条当前健康的能力观察覆盖全部所需特性，
不拼接无关能力的部分特性；没有能力声明的插件只能满足空特性要求。
绑定选择不授予业务权限。实际调用继续检查当前调用者、原始授权来源及逐级上限，
不把创建部署或订阅时的交互会话寿命当作永久执行委托。

HostService 不传递原用户 Token。临时不透明句柄固定实例/代次/原 Call，
子 Call 复用现有 Broker 的并发、计费、Blob 输入白名单和 unknown 物理围栏。
可选的预分配 Call UUID 及结构化 resource_id 支持未知结果核查；
传输完全丢失仍须使用预先保留的 ID 或现有管理接口核查，不能自动重新执行业务。
同一事务内的授权证明复用绑定实际事务 ID、SET LOCAL 随机标记和精确上下文，
提交后不能复用，叶子检查继续核验来源和凭证的当前有效期。

事件主题是全局授权命名空间，要求显式 ids/all；owned_by 不匹配主题。
订阅另校验归属。当前有权的新凭证可继续使用持久游标；历史创建凭证只供审计。
事件为至少一次交付，消费者用 event_id 或自身业务键去重，
不能把 ACK 当成外部副作用的恰好一次保证。恢复后旧订阅不自动恢复为活跃状态，
须重新授权并创建所需订阅。

## 验证依据与门禁

本地 PostgreSQL 测试使用一次性数据库，Host 测试使用真实回环 mTLS 传输；
它们验证元数据、授权和协议，不能替代 OCI 实际启动。
事件恢复测试实际终止并重新执行组件进程，原生快照测试实际进行 PostgreSQL binary COPY。
最终逐项结果、精确源提交和 CI 见阶段 PR 及目标 #51，不将编译成功或默认 ignored 计为实际执行。

| 必测范围 | 实际断言与证据入口 |
|---|---|
| 依赖图和固定选择 | 自环/间接环、并发绑定、16层边界、强请求边保留、可选缺失、撤权、健康/特性/配置/代次漂移、固定升级不重新发现；`dependencies_tests.rs` |
| Host 权限与故障 | 错误 peer/代次/原 Call、签名和审批交集、独立权限来源、父级/凭证撤权与到期、事务证明过期、子 Blob 白名单、固定 Skill、重复请求/unknown、流取消；`host_calls_tests.rs` 及相关模块 |
| EVENT-01 | 发布去重/冲突、未 ACK 断线重投、旧租约和重复 ACK、并发接管、退避与最大次数/死信、积压保留/容量拒绝、撤权和来源到期；`events_tests.rs`、`event_streams_tests.rs`、Host 事件测试 |
| 进程和存储故障 | 真实 SIGKILL/reexec 后 outbox 和租约恢复、事务回滚、领导连接丢失、锁等待后来源失效、COPY 损坏拒绝、恢复后旧凭证/租约/句柄不能复活 |
| 实际 OCI | 两个独立消费者经 Host 调用同一固定提供者、Skill 代理、事件去重/订阅/重复 ACK；required 提供者停用后消费者物理退出、预留唯一且全部回收；必须由 CI 的显式实际 OCI 阶段执行并输出 receipt |
| 既有回归 | 全 workspace、all-target Clippy、fmt、严格文档、协议、OAuth/WebUI 专项、真实 OCI/HTTP/XFS/恢复；精确 PR 与最终 main 的完整 CI 均须成功 |

集成中曾实际发现 XFS 测试在 Schema bootstrap 路径栈溢出。
依赖组合 future 改为先返回堆上状态再进入 worker poll，SQL、授权、事务及生产栈大小不变。
修复候选已有实际 XFS 完整 CI 证据；Host/事件集成还暴露了 HTTP 调用栈增长，
HTTP 核心分派同样使用先返回再 poll 的堆边界。每个最终提交均须以自身完整 CI 判定，
精确结果见目标 #51，不能由旧候选推定。
任何失败候选的日志均保留，后续成功不改写历史失败。

## 剩余验收边界

AWS 继续 **DEFERRED**。本地兼容对象存储不是正式 AWS/off_host 证据。
当前本地执行器没有宿主 root/containerd/cgroup 权限，实际 OCI 本地尝试按既有 root 门禁失败，
该项本地结果为 pending；远程 CI 必须实际执行，不能跳过或降低断言。

正式 RUN-03、完整 daemon/OS 重启、独立宿主/独立密钥恢复、长期容量与完整生产格式验收
仍由 #50 独立跟踪。小容器与合成组件预算不替代生产 4C16G 内核预留。
本轮不扩展至 M5/M6、TaskEngine、Workflow、MPR 或 Extension；
未实现的 profile 和 operation 继续明确拒绝。
