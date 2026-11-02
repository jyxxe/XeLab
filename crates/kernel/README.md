# XeLab Kernel：普通插件的首个运行闭环

这是基于现有 **2.8.0 契约**的 Rust 实现增量，crate 版本为 `0.1.0`。它覆盖 M1 的一个可运行子集，**不代表整个 M1 或 2.8 规格已经完成，也不是生产就绪版本**。现有 OpenAPI、Proto、Schema、SQL 基线保持原样；下面列出的边界以实际代码为准。

## 已实现与边界

| 模块 | 本次实现 | 尚未实现 |
|---|---|---|
| 元数据 | PostgreSQL 显式基线迁移及校验和、单控制器锁、事务内操作/审计/outbox | 在线升级迁移、多个控制器、自动备份恢复 |
| 身份与授权 | 空库引导独立 Owner；Argon2 密码；HMAC 摘要会话；登录限流；每次请求及调用完成前重验权限；会话撤销 | 子账户、API Key、OAuth、权限组/角色管理接口及完整委派语义；这些凭证在本实现中拒绝认证 |
| 制品 | 预导入的固定 OCI manifest digest；Ed25519 信封；原始清单摘要；契约文件摘要及内容一致性；严格 JSON/Schema 校验 | 在线拉取、image index 自动选平台、仓库登录、依赖安装、卸载及升级 |
| 控制器 | 安装 → 创建停用部署 → 显式批准 → 启用 → Describe/Initialize/Health → Ready → 排空停止；持久意图和有限重启 | 多副本、滚动升级、有状态数据迁移、动态配置更新 |
| 执行 | containerd/runc 真容器；cgroups v2 CPU/内存/PID；只读根目录、受限 tmpfs、无 Linux capabilities、隔离网络、工作负载 mTLS | 持久卷、外网/代理、HostService、GPU、微插件/MPR、Extension Plugin |
| 调用 | 普通插件直接、非流式、`side_effect=none` 调用；固定契约/可选固定部署；有界并发；账户调用配额；幂等记录；未知结果查询 | 等待队列、流、非纯调用、调用取消、Skill、Workflow、Batch、Schedule、Event |
| 运维 | HTTPS 管理 API、通用 CLI、探针、结构化错误日志、数据库审计、故障恢复测试 | WebUI、UDS 控制通道、指标/Tracing 导出、自动备份、数据与 AI 平台 |

只接受单副本、无状态、无依赖、无宿主权限申请、无卷、无 UI/路由、无扩展点的普通插件。业务 Schema 必须自包含；本版拒绝 `$ref/$id` 等引用，避免联网或读取宿主文件。并发池满时立即拒绝，不静默排队。未实现的管理操作返回明确错误，不返回模拟成功。

配置仍使用现有 `kernel.toml`；`unix_socket`、HostService 监听、持久存储配额后端、备份 URI 和 UI Gateway 在此子集中不提供对应服务。不要仅因配置被接受而认为这些能力已经启用。

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
xelabctl migrate --config /etc/xelab/kernel.toml --target 1
xelabctl bootstrap --config /etc/xelab/kernel.toml --owner-name admin
xelabd --config /etc/xelab/kernel.toml
```

`validate-config` 只验证结构和本地约束；不宣称宿主检查已经通过。迁移只作用于空库或校验同一基线；bootstrap 只允许空账户库，须在启动守护进程前执行。守护进程不会隐式迁移数据库。

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

本次可用 HTTP 操作：

`getLiveness`、`getReadiness`、`login`、`revokeSession`、`getNode`、`trustPublisher`、`getPublisher`、`revokePublisher`、`installPlugin`、`getInstallation`、`createDeployment`、`getDeployment`、`approvePlugin`、`getPluginApproval`、`setDeploymentEnabled`、`getInstance`、`getOperation`、`invokeCapability`、`getCall`。

## 恢复与安全语义

- 准入在事务中锁定节点预算，实例与 reservation 唯一关联；重复准入不重复扣账。只有运行时确认 task 和 cgroup 中进程已消失，才允许释放账本。
- 每次守护进程重启先撤出路由并停止旧 namespace 中的容器，再将未确认调用标记 unknown、归还并发配额、按原意图重新启动。实例资源预约保持不变，避免重启超售。这意味着当前版本的内核重启会中断插件服务。
- 安装异步执行前和提交时重新检查会话、发布者信任与安装权限。调用前和返回结果前都检查当前授权；RPC 委派仅绑定该实例/代次/调用，纯插件没有 HostService 权限。
- 丢失 PostgreSQL 控制器锁连接后停止接收业务写入/调用；结果写回带状态条件，不把恢复后被废止的调用写成成功。
- 停用先撤路由，再发送 Drain；超时后强制停止。存活探针使用清单期限与失败阈值，启动失败使用有限次数及退避，需人工停用/启用开始新代次。
- 容器不得访问宿主业务 API、其他插件或外网；内核发起的连接可以接收响应。配置、密钥及 Docker/containerd socket 不挂入插件。制品签名不替代沙箱。
- 发布者撤销阻止新的安装提交；不自动删除已经安装的制品或停止现有部署。需要停用时另行停用部署。

## 验证和发布门槛

默认测试包括严格 JSON、Schema、配置/秘密文件、权限匹配、制品路径/信封、OCI 隔离参数，以及真实 PostgreSQL 的会话撤销、重复迁移、并发准入、防超售和幂等释放。设置 `XELAB_TEST_DATABASE_URL` 后运行数据库测试；未设置会提示跳过，不能据此宣称数据库验证完成。

CI 另在一次性 Linux 主机运行 `tests/oci.rs`：真实 containerd + OCI 镜像 + cgroup + mTLS，经管理 Router 完成安装、批准、启用、调用、结果查询、幂等重放/冲突、控制器恢复、停用回收及撤销凭证。错误签名、错误实例代次证书和过期版本条件应被拒绝。该测试需显式 `XELAB_OCI_TEST=1`，默认忽略，**只允许在可销毁测试主机上手动运行**；完整命令见 [CI](../../.github/workflows/kernel.yml)。

托管 CI 容量低于规范的生产内核预留，因此 OCI 组件测试使用小的实际容器 cgroup 和独立测试数据库中的合成节点预算；生产 `xelabd` 没有绕过预留门槛的开关。它验证真实插件执行与控制面逻辑，不等价于已在合规容量宿主上验收 daemon 启动、HTTPS/CLI 全链路、长期负载和灾备。

合并后继续完成：合规宿主端到端验收及故障注入 → 非特权运行与运行时适配收敛 → 完整身份委派/SDK → 更多插件能力。新增能力须逐项补入验收，不以现有规格文档当作实现证据。

