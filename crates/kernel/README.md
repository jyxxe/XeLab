# XeLab Kernel：M1 单节点内核与身份闭环

这是基于现有 **2.8.0 契约**的 Rust 实现增量，crate 版本为 `0.1.0`。交付范围对应实施路线图的 **M1**：普通插件运行、身份继承、授权与管理闭环。现有 OpenAPI、Proto、Schema、SQL 基线保持原样。本轮补充 [M2 Secret 托管与租约](../../docs/implementation/m2-secrets.md)，其余 M2–M6 能力仍独立交付；合规容量宿主的生产发布验收仍须单独完成，见 [M1 验收记录](../../docs/implementation/m1-acceptance.md)。

## 已实现与边界

| 模块 | 本次实现 | 尚未实现 |
|---|---|---|
| 元数据 | PostgreSQL 显式基线迁移及校验和、单控制器锁、事务内操作/审计/outbox | 在线升级迁移、多个控制器、自动备份恢复 |
| 身份与授权 | Owner 引导、五种内置角色、Guest 自助、固定父级子账户与激活、三种继承模式、不可变权限组版本/授权、API Key 签发/轮换/源链撤销、会话/密码管理、树配额、审计与签名分页 | OAuth、Skill/Workflow 执行委托与缓存式授权优化；OAuth 凭证拒绝认证 |
| 制品 | 预导入的固定 OCI manifest digest；Ed25519 信封；原始清单摘要；契约文件摘要及内容一致性；严格 JSON/Schema 校验 | 在线拉取、image index 自动选平台、仓库登录、依赖安装、卸载及升级 |
| 控制器 | 安装 → 创建停用部署 → 显式批准 → 启用 → Describe/Initialize/Health → Ready → 排空停止；持久意图和有限重启 | 多副本、滚动升级、有状态数据迁移、动态配置更新 |
| 执行 | containerd/runc 真容器；cgroups v2 CPU/内存/PID；只读根目录、受限 tmpfs、无 Linux capabilities、隔离网络、工作负载 mTLS | 持久卷、外网/代理、HostService 其他方法、GPU、微插件/MPR、Extension Plugin |
| 调用 | 普通插件直接、非流式、`side_effect=none` 调用；固定契约/可选固定部署；有界并发；树配额及密钥链累计调用上限；幂等记录；未知结果查询；异常调用停止确认后回收 | 等待队列、流、非纯调用、调用取消、Skill、Workflow、Batch、Schedule、Event |
| Secret | AES-256-GCM 托管、元数据 API、轮换/撤销、部署绑定、工作负载 mTLS 租约 | 主密钥轮换、备份恢复 |
| 运维 | HTTPS 管理 API、通用 CLI、探针、结构化错误日志、数据库审计、故障恢复测试 | WebUI、UDS 控制通道、指标/Tracing 导出、自动备份、数据与 AI 平台 |

只接受单副本、无状态、无依赖、无宿主权限申请、无卷、无 UI/路由、无扩展点的普通插件。业务 Schema 必须自包含；本版拒绝 `$ref/$id` 等引用，避免联网或读取宿主文件。并发池满时立即拒绝，不静默排队。未实现的管理操作返回明确错误，不返回模拟成功。

配置仍使用现有 `kernel.toml`；`unix_socket`、持久存储配额后端、备份 URI 和 UI Gateway 在此子集中不提供对应服务。不要仅因配置被接受而认为这些能力已经启用。

## 构建与主机要求

```bash
cargo build --workspace --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
```

生成 `xelabd`、`xelabctl` 和参考插件 `xelab-echo`。Proto 直接由仓库原文件生成，构建工具内置 protoc；没有另写一套 RPC 定义。提交的 Cargo.lock 固定依赖解析结果。

运行需要独占的 Linux 节点、cgroups v2（cpu/memory/pids）、containerd/runc、`ctr`、`ip`、`nft` 和本机 PostgreSQL。守护进程以 root 运行以管理 OCI、网络和 cgroup；插件 UID/GID 为 65532，不获得宿主权限。本阶段仅接受 Unix socket 或数字回环地址连接 PostgreSQL。

必须满足：

- 内核预留至少 **4000 mCPU / 16 GiB**，系统预留另算，插件池为剩余容量；不降低现有规范门槛。
- 示例配置另预留 2 CPU / 4 GiB，因此宿主容量须高于 6 CPU / 20 GiB 才有插件资源。容量不足时启动失败。
- 使用独立 containerd namespace、独立且不与宿主路由冲突的 IPv4 CIDR（/16～/28）。
- 本版采用 `/sys/fs/cgroup/xelab`。不要在已有其他管理器使用该路径的共享宿主上运行。
- HTTPS 服务证书须匹配 `public_origin`；工作负载 CA 使用独立的 EC P-256 证书及 PKCS#8 私钥。工作负载证书有效期 1 小时，约 30 分钟更新；插件须重新加载证书，参考插件每 5 分钟重建监听。
- DSN、HMAC key、master key、TLS/CA 私钥文件仅属运行用户，普通文件、无符号链接/硬链接、权限不宽于 0600。HMAC key 至少 32 字节，master key 恰好 32 字节。原始凭证不写日志或元数据。

从 [配置示例](../../docs/contracts/examples/kernel.toml) 复制到宿主配置目录，填写实际地址及秘密文件路径。用 `openssl rand -out <file> 32` 在 `umask 077` 下生成随机密钥；密码通过 stdin 输入，不放进命令行。

```bash
xelabctl validate-config --config /etc/xelab/kernel.toml
xelabctl migrate --config /etc/xelab/kernel.toml --target 3
xelabctl bootstrap --config /etc/xelab/kernel.toml --owner-name admin
xelabd --config /etc/xelab/kernel.toml
```

`validate-config` 只验证结构和本地约束；不宣称宿主检查已经通过。迁移目标 2 在空库建立原始基线，或将已校验的 v1 数据库升级，随后创建调用来源计费表；重复执行只校验版本与校验和；bootstrap 只允许空账户库，须在启动守护进程前执行。守护进程不会隐式迁移数据库。

## 管理与插件示例

CLI 的 `api` 子命令从原 OpenAPI 读取 operationId、路径及方法。所有服务请求使用 HTTPS；自签测试服务证书可以用 `--ca-file` 指定，不提供跳过校验选项。JSON 输出到 stdout，错误摘要到 stderr。

```bash
xelabctl api login --endpoint https://kernel.example.org:8443 \
  --body-file login.json --ca-file server.crt
xelabctl api getNode --endpoint https://kernel.example.org:8443 \
  --token-file session.token --ca-file server.crt
```

`login.json` 为 `{"account_id":"<bootstrap 输出的 UUID>","password":"<密码>"}`，按秘密文件保护。将首次返回的 `access_token` 保存到当前用户拥有的 0600 token 文件；不要把 token 放进仓库。会话有效期 1 小时，登录和会话撤销均记录审计。

制作参考插件的顺序：

1. 生成发布者 Ed25519 密钥，将公钥通过 `trustPublisher` 登记，取得 `publisher_id`。
2. 生成清单和四个契约文件；先 `cargo build --bin xelab-echo --locked`，再构建镜像。示例 Dockerfile 用于 Ubuntu 24.04/同架构构建产物，Docker 只用于示例镜像打包，内核运行不依赖 Docker。
3. 管理员将镜像导入配置指定的 containerd namespace，取得**平台对应的 image manifest** 摘要（不是多平台 index 摘要），再给该镜像添加包含 `@sha256:...` 的不可变名称。
4. 对已确定的镜像摘要和原始清单签名，提交 `installPlugin`。安装成功不自动启用。

```bash
python3 examples/echo/build_artifact.py --publisher-id <publisher-uuid>
docker build -t docker.io/local/xelab-echo:1.0.0 -f examples/echo/Dockerfile .
docker save docker.io/local/xelab-echo:1.0.0 -o examples/generated/echo.tar
sudo ctr -n xelab images import examples/generated/echo.tar
sudo ctr -n xelab images list
sudo ctr -n xelab images tag docker.io/local/xelab-echo:1.0.0 \
  docker.io/local/xelab-echo@sha256:<manifest-digest>
python3 examples/echo/sign_artifact.py \
  --manifest examples/generated/xelab/plugin.json \
  --image docker.io/local/xelab-echo@sha256:<manifest-digest> \
  --key publisher.key > install.json
```

之后按以下顺序调用，JSON 模型仍以原契约为准：

| operationId | 请求/检查 |
|---|---|
| `installPlugin` | `install.json`；轮询 `getOperation`，获得 installation_id |
| `createDeployment` | installation_id、display_name、enabled=false、limits、config={}、volume_ids=[]；返回 operation |
| `approvePlugin` | deployment_id 路径；`{"rules":[],"egress":[]}`，必须传当前 If-Match |
| `setDeploymentEnabled` | `{"enabled":true}`，使用更新后的 revision；轮询 operation 至成功 |
| `invokeCapability` | 清单中的完整 contract、`input:{"message":"hello"}`、可选 provider_deployment_id |
| `getCall` | 用调用结果或错误的 resource_id 查询，unknown 不能当作成功或自动重试 |
| `setDeploymentEnabled` | `{"enabled":false}`；operation 成功时容器已经停止且资源已释放 |

路径参数使用 `--path deployment_id=<uuid>`；版本条件示例为 `--if-match '"rev-2"'`。每次读资源取当前 revision，不猜版本。写请求建议显式提供 `--idempotency-key <uuid>` 并保留，网络重试复用同一 key；相同 key、不同请求返回冲突。CLI 未传时生成新 key，因此重新运行命令不等于重放原请求。

普通插件、节点与探针可用 HTTP 操作：

`getLiveness`、`getReadiness`、`login`、`revokeSession`、`getNode`、`trustPublisher`、`getPublisher`、`revokePublisher`、`installPlugin`、`getInstallation`、`createDeployment`、`getDeployment`、`approvePlugin`、`getPluginApproval`、`setDeploymentEnabled`、`getInstance`、`getOperation`、`invokeCapability`、`getCall`。

身份可用操作按既有 operationId 分组：

| 对象 | 操作 |
|---|---|
| 账户 | `listAccounts`、`getAccount`、`updateAccount`、`archiveAccount`、`createChildAccount`、`activateAccount`、`changePassword`、`updateInheritance`、`getEffectivePermissions` |
| 权限 | `listRoles`、`listPermissions`、`listGroups`、`createGroup`、`getGroup`、`archiveGroup`、`publishGroupVersion`、`getGroupVersion`、`listGrants`、`createGrant`、`getGrant`、`revokeGrant`、`checkAuthorization` |
| 凭证 | `listApiKeys`、`createApiKey`、`getApiKey`、`revokeApiKey`、`rotateApiKey`、`listSessions`、`revokeSession` |
| 配额/审计 | `getQuota`、`updateQuota`、`listAuditEvents` |

身份操作也通过 `xelabctl api <operationId>` 调用。`createChildAccount` 在 `--path account_id=<parent>` 下提交 `{"display_name":"child","inheritance":{"mode":"all_snapshot"}}`，首次返回待激活账户及 24 小时激活令牌；`activateAccount` 提交该令牌与新密码。服务账户不签发交互密码。归档保留原父级，不能迁移账户树。

`createApiKey` 使用 `--path account_id=<owner>`，例如 `{"name":"echo only","inheritance":{"mode":"selected","rules":[{"action":"capability.invoke","resource":{"kind":"capability","match":"ids","ids":["org.xelab.echo@1.0.0"]}}]},"conditions":{"expires_at":"<未来 RFC3339 时间>","max_calls":100}}`。签发者须已有对应能力权限。返回 `credential` 元数据与只出现一次的 `secret`；同幂等键重放返回 409 和已有资源 ID，不再次泄露秘密。激活令牌和 API Key 原文均不落库。

独立 Python 插件见 [echo-python](../../examples/echo-python/README.md)：仅使用现有 Proto 生成代码，与 Rust 插件复用完全相同的契约文件和摘要，使用不同镜像/安装/部署，经同一管理 API 接入。

## 恢复与安全语义

- 准入在事务中锁定节点预算，实例与 reservation 唯一关联；重复准入不重复扣账。只有运行时确认 task 和 cgroup 中进程已消失，才允许释放账本。
- 每次守护进程重启先撤出路由并停止旧 namespace 中的容器，再将未确认调用标记 unknown、归还并发配额、按原意图重新启动。实例资源预约保持不变，避免重启超售。这意味着当前版本的内核重启会中断插件服务。
- 安装异步执行前和提交时重新检查会话、发布者信任与安装权限。调用前和返回结果前都检查当前授权；RPC 委派仅绑定该实例/代次/调用，纯插件没有 HostService 权限。
- API Key 及子账户使用实时来源图：同一授权路径上的父级/凭证/固定上限取交集，独立有效来源取并集。快照持久保存真实来源 grant，后来新增权限不会扩张快照，替代 grant 不会复活已撤销来源。权限组仅为不可变模板，读取或引用模板不产生权限。
- 身份写入持独占授权事务锁，认证与调用准入/结果提交持共享锁；不缓存有效权限，撤权后新准入和结果交付重新判定。最后一个有效独立 Owner 不能被禁用、归档或撤成不完整权限。
- 子账户数、层级、API Key、日调用和并发调用按祖先链约束；密钥派生分支共享来源密钥的累计调用预算。调用超时/返回未知或 HTTP 处理任务丢失时，控制器先撤路由、确认旧进程终止，再归还并发配额并重建原实例。
- 浏览器会话 cookie 为 `Secure`、`HttpOnly`、`SameSite=Lax`、`__Host-` 前缀，cookie 写请求必须匹配配置的 Origin；CLI 使用 Bearer。密码修改撤销交互会话，不撤销独立 API Key。
- 丢失 PostgreSQL 控制器锁连接后停止接收业务写入/调用；结果写回带状态条件，不把恢复后被废止的调用写成成功。
- 停用先撤路由，再发送 Drain；超时后强制停止。存活探针使用清单期限与失败阈值，启动失败使用有限次数及退避，需人工停用/启用开始新代次。
- 容器不得访问宿主业务 API、其他插件或外网；内核发起的连接可以接收响应。配置、密钥及 Docker/containerd socket 不挂入插件。制品签名不替代沙箱。
- 发布者撤销阻止新的安装提交；不自动删除已经安装的制品或停止现有部署。需要停用时另行停用部署。

## 验证和发布门槛

默认测试包括严格 JSON、Schema、配置/秘密文件、权限条件交集、制品路径/信封、OCI 隔离参数，以及真实 PostgreSQL 的 AUTH-01～09、会话撤销、重复迁移、并发准入、防超售和幂等释放。设置 `XELAB_TEST_DATABASE_URL` 后运行数据库测试；测试 DSN 须有创建一次性数据库的权限。未设置会提示跳过，不能据此宣称数据库验证完成。

CI 另在一次性 Linux 主机运行 `tests/oci.rs`：真实 containerd + Rust/Python 两个 OCI 镜像 + cgroup + mTLS，经管理 Router 完成安装、批准、启用、调用、幂等重放/冲突、两处恢复注入、调用配额回收、停用及撤权；实际执行 CPU 压力、内存 OOM、只读根目录/出网拒绝，并以真实 HTTPS 服务运行 `xelabctl`。错误签名、错误实例代次证书和过期版本条件须被拒绝。该测试需显式 `XELAB_OCI_TEST=1`，默认忽略，**只允许在可销毁测试主机上手动运行**；命令见 [CI](../../.github/workflows/kernel.yml)。

托管 CI 容量低于规范的生产内核预留，因此 OCI 组件测试使用小的实际容器 cgroup 和独立测试数据库中的合成节点预算；生产 `xelabd` 没有绕过预留门槛的开关。它验证真实插件执行、HTTPS/CLI 和控制面逻辑，不等价于合规容量宿主上的完整 daemon 启动、4C16G 保留量在持续压力下的有效性、长期负载或灾备验收。

发布前补齐合规宿主容量验收；后续按 M2 存储与可恢复运行、M3 Skill/OAuth/WebUI、M4–M6 通信/自动化/生态顺序演进。身份求值和全局授权写锁优先保证正确性，大账户树下的吞吐须实测再优化。



## PR #3 审查修复

- 调用准入在授权/调用事务锁内选择一个可用的真实来源路径，将请求凭证链与该路径全部 Grant 的来源凭证链合并检查 `max_calls`。独立来源取并集；耗尽的 Key 不阻断其他合法来源。`call_credential_charges` 与 Call 原子写入，幂等成功重放不重复计费，撤销 Grant/Key 不退还已用额度。轮换仍扣除已使用预算。
- 自授 Grant 证明签发权限时，忽略同账户其他非引导自授绑定，只使用独立来源（外部 Grant、父级继承或初始 Owner）。这些自授绑定仍在最外层保留权限/来源记录；撤销独立来源后不能互相维持权限。此优化针对同账户自授，其他来源图仍受现有深度/路径边界限制。
- 控制器启用与停用清理分别保存 `(created_at,id)` 游标与上界，每轮各最多 16 项，并在项间检查 250 ms 时间片。Probe 按实例代次复用、分批回收；已清理禁用记录不反复协调。完整行为、回归与单项延迟限制见 [Controller 稳定化](../../docs/implementation/m1-acceptance.md#controller-稳定化pr-4-遗留项)。

升级前备份并停机，执行上面的 `--target 3` 后再启动。旧二进制拒绝 v3 元数据，不能直接回退。v1 已有的凭证/账户派生调用继续计入预算；旧版本未记录的实际 Grant 来源无法事后精确重建，因此已有受此漏洞影响的限次来源 Key 应在升级时撤销后重新签发，不把旧账本当成完整历史计费证据。

## M2 存储后端预检

`xelabctl storage-check --config /etc/xelab/kernel.toml` 只读验证平台卷目录的
XFS project quota 强制执行条件。后端创建/扩容原语和真实文件系统测试见
[M2-A 操作说明](../../docs/implementation/m2-storage.md)。本阶段不启用公开
Volume API 或容器卷挂载；M1 的持久卷拒绝策略继续生效。
启动 readiness 等待 active/inactive 两条队列各自完成首轮固定水位扫描；
首批成功不表示恢复完成。全局旧进程围栏完成后不因后续批次错误重复执行。
