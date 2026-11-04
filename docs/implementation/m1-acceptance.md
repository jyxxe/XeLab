# M1 实现与验收记录

[实施目录](README.md) · [运行指南](../../crates/kernel/README.md) · [规范性验收矩阵](acceptance.md)

本记录描述 M1 的代码与测试证据，不改变 2.8 规范。PR #2 提供普通插件运行基础，本增量补齐身份与授权闭环、独立插件互操作及异常调用回收。**实现交付与生产发布验收分开记录**；本文件不能代替对应提交的绿色 CI。

## 交付边界

- 单节点 PostgreSQL + containerd/runc；安装、部署、实例分离；固定镜像摘要和发布者签名；普通插件的单副本、无状态、非流式、无副作用调用。
- Guest 固定自助能力、五种角色、权限组及不可变版本、授权来源、固定父级账户树、激活与密码、会话/API Key、快照/选定/实时继承、来源撤销、原子树配额、逐对象可见性、审计。
- 两个独立插件实现：Rust 参考实现与仅依赖生成 Proto 的 Python 实现。共享同一契约文件和摘要，镜像、插件 ID 与部署独立；安装第二种实现无需修改核心。
- HTTPS 管理 API 与通用 CLI。实际支持 operationId 见运行指南；其余操作明确拒绝，不返回空成功。

核心实现对应 `policy.rs`、`identity.rs`、`auth.rs`、`api.rs`、`broker.rs`、`controller.rs`。没有更改 OpenAPI、Proto、JSON Schema、数据库基线或已有 2.8 设计文档。

## 自动化证据

真实数据库用例在 [`tests/identity.rs`](../../crates/kernel/tests/identity.rs)，每个用例使用独立数据库并经 HTTP Router 请求。不得用未设置 DSN 时的跳过结果替代数据库验收。

| 编号 | 测试函数/检查点 | 覆盖内容 |
|---|---|---|
| AUTH-01/02 | `auth_01_02_guest_self_service_and_group_cannot_escape_parent` | Guest 自助子账户与 Key；拒绝平台读取/权限扩张；绑定高角色仍受父级上限约束；Key 不自动取得自助动作 |
| AUTH-03 | `auth_03_snapshot_follow_and_independent_provenance` | 新增权限只扩张 follow_parent；两个独立来源保留有效分支；撤销来源后新 grant 不复活旧快照 |
| 来源部分收缩 | `snapshot_tracks_partial_source_withdrawal_without_borrowing_new_grants` | 来源由 A/B 收缩至 A 后，后来新增的独立 B 授权不能填补旧快照的 B 缺口 |
| 子账户直接授权 | `child_direct_grants_obey_selected_sources_and_ceiling`、`child_direct_grants_obey_snapshot_sources_and_ceiling`、`child_direct_grants_preserve_follow_parent_alternatives` | 角色/组直接 Grant 纳入实时来源和派生 Key；父级撤权及固定范围限制；独立来源分别撤销；传递到孙账户；旧 Key 快照不被替代来源复活 |
| 源密钥上限 | `child_direct_grants_remain_bounded_by_issuer_key` | 即使父级和直接 Grant 更宽，子账户仍受原签发 Key 范围约束；吊销源 Key 后直接 Grant 不能恢复认证 |
| AUTH-04/05/06 | `auth_04_05_06_source_revocation_idempotency_and_secret_storage` | K1→K2/子账户来源链失效，独立 Key 保留；一次性响应重放 409；元数据/审计不含 secret；归档不改父级 |
| AUTH-07/08 | `auth_07_08_object_visibility_last_owner_and_stale_etag` | 他人对象隐藏；最后 Owner 保护；过期 ETag；游标不能换主体使用 |
| AUTH-09 | `auth_09_concurrent_tree_and_key_quotas_are_atomic` | 并发创建子账户/API Key 时额度内恰好成功，不超售 |
| 补充身份 | `key_rotation_password_and_parent_disable_preserve_boundaries` | 密钥轮换保留旧上限与源链；父级禁用生效；改密使会话失效但保留独立 Key |
| 模板与条件 | `immutable_group_templates_do_not_become_authority_sources` | 发布不改旧版本；公开模板不授权；模板归档不抹去来自真实 grant 的快照；拒绝非法 action/CIDR；来源网段限制 |
| 浏览器边界 | `browser_session_cookie_requires_same_origin_for_mutation` | 安全 cookie 属性；缺少或不匹配 Origin 的 cookie 写请求拒绝 |

[`tests/postgres.rs`](../../crates/kernel/tests/postgres.rs) 验证原始基线迁移、会话撤销、并发实例准入及幂等资源回收。单元测试覆盖严格解析、权限条件交集、配置/秘密文件、签名信封、OCI 参数等。

子账户的继承来源与显式直接 Grant 在同级取并集，两条路径均受当前父级、固定规则范围及原源凭证限制。固定模式的继承路径继续检查创建时的来源；后来显式签发的合法直接 Grant 使用自身来源，不借已失效的继承路径授权。撤销直接 Grant 只移除该路径，仍有效的继承/其他直接来源保留；已签发的 Key 继续冻结当时的真实来源，新直接 Grant 不会复活旧 Key。

特权用例 [`tests/oci.rs`](../../crates/kernel/tests/oci.rs) 的 `signed_oci_plugin_runs_through_the_management_api_and_recovers` 使用真实 containerd、两个镜像、网络隔离、cgroups v2 与 mTLS：

| 编号 | 注入/断言 | 证据边界 |
|---|---|---|
| RUN-01 | DB 启用意图提交后、创建 OCI 前取消控制循环，再启动；实例 ID 与唯一 reservation 不变 | 已有持久意图恢复路径 |
| RUN-02 | 手动完成 OCI 创建、未提交 runtime_id 即重启控制循环；旧进程受控停止并按原 ID 重建 | 无重复实例或资源预留 |
| RUN-03 部分 | 实际 CPU 限额与 throttled 计数、内存上限/OOM、PID 限制、另一插件存活；只读根目录及出网拒绝 | 托管 CI 使用小 cgroup；不等于生产 4C16G 保留量压力验收 |
| RUN-07 | 无 WebUI，启动实际 HTTPS 管理服务，执行构建出的 `xelabctl api invokeCapability` 并核对输出 | CLI、TLS、身份、调用同链路；不是完整宿主部署验证 |
| 互操作 | 同一契约经两个独立实现返回相同业务结果；错误签名/错误实例代次证书拒绝 | 含真实生成的 Python gRPC 绑定 |
| 调用额度/恢复 | `max_calls=1`、幂等重放不再次扣账；注入无人处理的过期 running 记录，确认进程停止后回收 reservation/祖先并发额度，再恢复相同实例 | 结果未知不能作为成功或自动重发依据 |

## 执行与复核

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

数据库测试设置 `XELAB_TEST_DATABASE_URL`，账户需有创建测试数据库权限。特权测试显式设置 `XELAB_OCI_TEST=1`、`XELAB_TEST_IMAGE`、`XELAB_TEST_PYTHON_IMAGE`，镜像先生成、构建、导入 `xelab-ci` namespace，再执行 ignored 用例；完整可复现步骤见 [Kernel M1 CI](../../.github/workflows/kernel.yml)。只能在可销毁 Linux 主机上运行，该测试会管理 cgroup、网络、容器和测试数据库。

审阅时检查对应 PR 最新提交的全部 CI 步骤成功，包括真实 OCI 步骤及最后的 tracked-files 检查。格式修复、测试失败或仅跳过特权用例的运行均不算通过。

## 兼容与剩余发布门槛

- 从 #2/#3 升级继续保留原 SQL 基线及校验和，显式执行 `migrate --target 2` 添加内部调用来源计费表，迁移与版本记录原子提交，不运行破坏性迁移。启动幂等补全内置角色定义，既有 Owner、会话、安装、部署与非敏感幂等账本仍可读取；角色动作与既有权限目录一致。
- 新身份能力使用现有表和 `grant_provenance.source_rules` 保存来源路径。旧二进制不能识别新增 API Key/子账户认证语义；回退前停机备份并停用新凭证，不把回退当作等价授权语义。原始秘密始终不能恢复。
- 生产 `xelabd` 必须实际满足内核至少 4 CPU/16 GiB、另加系统预留和插件池；不能使用组件测试的合成预算作为容量证明。仍需在合规独占宿主运行守护进程、并发饱和 CPU/内存并验证父级合计上限、管理可用性及真实保留量。当前环境没有完成该项，**RUN-03 完整生产验收未关闭**。
- 独占授权写锁与有界来源图优先保证正确性；大账户树/高频授权更改吞吐、长期稳定性尚未给出性能承诺。内核重启当前会中断并重建插件进程。
- 持久卷、Blob、备份/灾备、通用升级迁移、OAuth、Skill/WebUI、HostService/依赖/事件、Workflow/AI、微插件、Extension Plugin、多节点/GPU不属于本次交付；对应 RUN-04～06、TASK/STREAM/EVENT/BACKUP/EXT 验收不标记完成。

## PR #3 审查回归

| 问题 | 回归证据 |
|---|---|
| Grant 来源 Key 调用预算 | OCI 用例中的 `grant_source_budget`：K1(max_calls=1) 给子账户直接授权，子账户用独立 session 调用；首次成功、同键重放不重复计费、新键拒绝；独立授权可继续调用且不扣 K1；撤销来源后旧重放也拒绝，历史用量仍为 1 |
| 自授 Grant 递归 | `self_issued_grants_have_linear_proofs_and_live_revocation`：2/8/16 条自授 Grant，各有独立来源路径，调用证明数为 N+1；签发/求值有 10 秒超时，撤销引导来源后不循环续权 |
| Deployment 饥饿 | `controller_reconciles_deployment_1001_after_disabled_prefix` 实际运行控制器并确认尾部进入 reconcile；`deployment_sweep_passes_disabled_prefix_and_handles_deletions`：1000 条禁用前缀、同一 created_at、游标行删除、尾部归档与并发新增；后续可用部署仍被取出，上界外新增留给下轮 |
| 升级兼容 | `call_charges_migrate_from_v1_without_rewriting_baseline`：v1 未升级拒绝启动；v2 重复迁移成功，原始基线校验和不变 |

本修复不关闭 RUN-03 生产宿主压力验收。自授回归记录路径数和耗时，不代表完整授权图或大规模数据库的吞吐基准；v1 未记录的 Grant 计费来源也不能追溯恢复，升级处理见运行指南。

## 部署异步操作结果

createDeployment 和 setDeploymentEnabled 的成功结果按现有契约返回完整 Deployment，
包括 id、完整 spec、revision、观察代次、阶段及条件；不再新写入只有 deployment_id
的片段。对象投影与 getDeployment 共用校验。客户端应读取 resource_id 或 result.id。
已存储终态结果及幂等响应不回写，旧操作仍保留其原结果。

控制器仅完成仍待处理、且当前 enabled 意图与本次观察一致的操作，并核对生成结果
时的完整部署快照。旧启停观察不能完成相反的新意图，迟到处理不改写终态。
本地真实 HTTP/PostgreSQL 已验证创建、启用/禁用结果符合 Deployment、默认字段完整，
相反意图保持 pending，终态不重写。该数据库用例的阶段是元数据夹具，不是 OCI
健康/停止证明；真实 OCI 用例另在每个成功创建/启停操作轮询处校验结果，远端待执行。
这项修复不代替 RUN-03 独立宿主验收。

## Controller 稳定化（PR #4 遗留项）

- 启用部署与停用/归档清理使用独立的 `(created_at,id)` 游标和固定上界，每轮各最多处理 16 项。每项结束后检查 250 ms 时间片；未消费的分页记录保留给下轮，单项错误仍推进游标。已完成清理且无实例/待办的禁用记录不再反复协调。
- 每个批次先回收过期调用、处理安装，再进行健康/清理工作；大量禁用部署不会占用启用队列的批次。新增、启用发生在游标之前的记录在下一次 sweep 可见；游标行或上界行被删除不导致重头或永久饥饿。
- Probe 只在存在启用实例时建立，按实例 ID/代次绑定；同代次保留健康失败计数和期限，新实例清空旧探针状态。停用/归档协调立即删除，另用有序游标每轮最多核对 64 个 Probe，回收被删除或不再启用的部署。实例重启预算/退避继续来自数据库，不因 Probe GC 重置。
- PostgreSQL 回归：实际协调第 1001 个启用部署且首轮禁用处理不超过 16；连续多轮最终清完禁用前缀；1100 个启用记录在分页中删除、归档、停用、持续新增；40 轮共 5200 个 Probe 的 churn 与活动健康状态保留。单元测试验证同代次保留/重新部署重置。OCI 回归在 1200 条禁用积压下验证过期 Call 回收及稳定实例恢复先于清理完成。
- 兼容：无 Schema/Proto/OpenAPI/SQL 或迁移版本变化，仍使用 v2 元数据。升级需停止旧控制器后启动新二进制，原有恢复围栏继续生效。

**限度：**250 ms 是启动下一项工作的时间片，并非整轮硬截止；不会取消进行中的 OCI/数据库副作用。单项 Drain/宿主命令、安装、数据库锁等待及首次启动围栏仍可能较慢。批次数有界不等于 SQL 扫描成本恒定（当前无新增索引），健康间隔也随启用实例总数增长。此变更修复全量同步扫描和 Probe 生命周期，不构成生产容量或健康延迟 SLA；RUN-03 仍待独占合规宿主验证。

## 宿主容量发现修复

`host_capacity` 在物理内存和可用 CPU 基础上，逐级取当前 cgroup 及可见祖先
`memory.max` / `cpu.max` 的最小硬上限，CPU 按毫核向下取整。非法限额、
缺失下级控制器文件及隐藏祖先的 cgroup 子树挂载会拒绝启动；运行时管理
固定宿主层级，不将容器内的 `/proc/meminfo` 当作可分配预算。真实宿主根
cgroup 没有限额文件是 Linux 的正常行为，单独覆盖。

四项准入单元回归覆盖祖先限额、毫核取整、损坏/缺失文件、子树挂载与
真实根层级。该修复不替代 RUN-03；云工作区的 4 CPU/16 GiB 限额不足以
同时满足内核保留、系统保留及插件负载，正式宿主验收仍未执行。
