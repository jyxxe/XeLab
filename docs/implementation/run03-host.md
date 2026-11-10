# RUN-03：独占宿主压力验收手册

本页保留的4C16G容量要求及既有证据属于 `standard` profile；[compact 1C512MiB](compact-node.md) 有独立准入与整机验收，不复用本页容量结论。

状态：**测试准备，不是验收通过**。工具不修改 daemon 的资源校验，不建立合成 node_budget，也不会自动输出 RUN-03 passed。需由执行人审阅完整证据并逐项签署。当前开发环境没有合规、经授权的独占 Linux VM，真实验收仍待执行。

## 环境与安全边界

仅在允许销毁的独占 VM/裸机执行；建议至少 8 vCPU / 32 GiB，具体须超过内核 4 vCPU / 16 GiB、系统预留和插件池之和。需要 cgroups v2、containerd/runc、ip/nft、PostgreSQL 16、psql、Python 3.11+、Rust 构建工具。不要在生产宿主或共享 `/sys/fs/cgroup/xelab` 的机器上运行。

[`run03.py`](../../tools/acceptance/run03.py) 提供：

- `preflight`：只读检查真实可用 CPU/内存、当前 cgroup 及祖先硬限额、内核/系统预留、容器环境、主机二进制和 containerd socket。通过仅表示具备初步条件；不代表网络、存储、守护进程与压力验收成功。
- `collect`：核实 PID 确实运行指定配置的 `xelabd` 且位于 kernel cgroup，记录二进制 SHA256、真实父级限额/用量、管理 API 状态/延迟、只读一致性数据库快照。核验及每个样本前后检查 PID 与进程起始时间；进程消失或 PID 被复用时记录错误并停止采集。对资源配置与实测宿主不一致、资源账本/祖先并发账本不一致输出错误。这是离散检查，不能单独证明采样间没有变化或 API 必然由该进程提供。
- `stress`：通过 `ctr tasks exec` **仅在指定现有插件实例内**运行有时间上限的 CPU/OOM/PID 工作负载；核对 task PID、真实 cgroup、OCI 标签/代次、实例限额/预留账本和父级硬限制；有限但超预算同样拒绝。远端先 SIGSTOP，经 pidfd 核实后才释放压力。使用 Python 参考镜像。OOM 非零退出既可能是预期终止也可能是故障，必须检查 `memory.events` 与恢复状态，不能当作通过。
- `calls`：通过现有 OpenAPI 的 invokeCapability 发起有并发、数量与时间上限的调用；记录状态码、错误码和延迟，不保存输入/结果明文。截止时未执行项明确记为 not_attempted；正常返回不代表并发上限已经触发。

除 preflight 外需 `--isolated-test-host`，表示执行者已确认目标是授权测试主机。工具不自动迁移、引导账户、安装镜像、启动/杀死 daemon 或修改部署意图。stress 结束（包括正常结束）会清空事先固定的测试实例 cgroup，终止该实例及所有子进程。仅当 ctr 已确认正常完成时不设置冻结围栏，Controller 可随后重建；超时/异常/非零退出保留冻结围栏并报告 quarantined，必须由操作员处置。不会合并代码或操作生产环境。

## 1. 固定版本、部署与启动

1. 在待验收的准确 commit 上运行 `cargo fmt --all -- --check`、完整 PostgreSQL 测试、Clippy，并按 Kernel CI 执行 OCI 组件测试。记录 SHA、CI URL 和结果；组件测试仍不等于本验收。
2. 按 [Kernel 运行指南](../../crates/kernel/README.md) 建立独立数据库、私钥、TLS、配置、发布者和 Rust/Python 签名镜像；只用测试凭证。DSN/令牌文件 0600，禁止提交仓库。配置保留实际 4C16G 及独立系统预留。
3. 执行显式 v2 迁移与 bootstrap，保存 owner UUID；使用正式 `xelabd` 入口启动。工具不会自动覆盖已有结果，建议每次使用新目录。

```bash
umask 077
mkdir run03-evidence
cargo build --workspace --bins --locked
python3 tools/acceptance/run03.py preflight \
  --config /etc/xelab/kernel.toml --output run03-evidence/preflight.json
# preflight 返回 0 后，另按运行指南执行 migrate/bootstrap。
target/debug/xelabd --config /etc/xelab/kernel.toml >run03-evidence/daemon.log 2>&1 &
RUN03_DAEMON_PID=$!
```

使用 HTTPS 登录，把 access_token 存入 0600 的 `run03-session.token`。按运行指南通过 `installPlugin → createDeployment → approvePlugin → setDeploymentEnabled` 建立至少两个 Python 插件部署；一个承受压力，另一个用于验证隔离。管理写入使用当前 If-Match 和唯一幂等键。使用独立 namespace，记录安装摘要/发布者/部署 ID。

从 `ctr --namespace <配置 namespace> containers list` 的 `org.xelab.deployment` 标签核对实例 ID，再调用公开 `getInstance` 核对身份和 limits。勿猜测 ID，勿把 deployment UUID 当 instance UUID。整个宿主必须是测试环境。stress 还要求插件父级 `pids.max` 为有限正值且
不低于目标实例 PID 上限；当前 M1 不自动配置该父级 PID 上限，操作员须在
隔离测试宿主先设置并记录该测试环境约束，不能宣称这是内核已实现的策略。
父级 CPU/内存必须与实际宿主和 node_budget 推导的插件池完全一致，swap 为 0。
工具通过只读 repeatable-read 快照核实目标实例和 reservation，不凭人工抄录值放行。

```bash
RUN03_INSTANCE=<经标签和getInstance核对的实例UUID>
python3 tools/acceptance/run03.py collect --isolated-test-host \
  --config /etc/xelab/kernel.toml --daemon-pid "$RUN03_DAEMON_PID" \
  --token-file run03-session.token --ca-file /etc/xelab/tls/server.crt \
  --instance "$RUN03_INSTANCE" --seconds 30 --output run03-evidence/baseline.json
```

## 2. 持续饱和、OOM、PID 与管理可用性

先让所有测试插件的实例配额总和接近实际插件池，但不能超出准入账本。单个 500mCPU 插件只能证明自己的限额，不能证明父级饱和；需提高测试部署限额或增加部署，记录每个实例限制。超过节点池的新部署必须等待/拒绝，不能借用内核预留。

在一个终端运行 collect 900 秒，在其他终端对测试插件分别运行 CPU 工作负载。workers 应足以使每个插件达到 CPU 限额且不超过 PID 预算。以下命令演示单实例；每实例使用独立输出文件。

```bash
python3 tools/acceptance/run03.py collect --isolated-test-host \
  --config /etc/xelab/kernel.toml --daemon-pid "$RUN03_DAEMON_PID" \
  --token-file run03-session.token --ca-file /etc/xelab/tls/server.crt \
  --seconds 900 --output run03-evidence/under-load.json

python3 tools/acceptance/run03.py stress --isolated-test-host \
  --config /etc/xelab/kernel.toml --daemon-pid "$RUN03_DAEMON_PID" \
  --instance "$RUN03_INSTANCE" --fault cpu --workers 8 --seconds 600 \
  --output run03-evidence/cpu-instance-1.json
```

随后在持续管理采样期间分别运行 `--fault oom --seconds 30` 和 `--fault pids --seconds 30`。每次使用新 output。工具保存 exec-id，超时/异常时显式向远端 exec 发送
SIGKILL，再通过预先打开的 cgroup 描述符 kill 全部子进程并核实 populated=0；
仅在不能确认 exec 正常完成时先冻结，保留隔离状态。
清理不会按路径误杀已替换的 cgroup，也不会主动解冻不确定的旧组。quarantined=true
时 runc 可能把空组仍视为 PAUSED，普通管理停用不保证可完成；停止本轮并保留证据，
由操作员停机/销毁 VM，不得直接解冻后继续验收。清理失败
写入 safety_blocked 并返回 3，必须停止本轮验收，保留现场并人工停机/销毁 VM。
SIGINT/SIGTERM 触发清理；工具被 SIGKILL、宿主掉电或内核阻塞时仍需操作员处置。
先检查 after（清理前计数）与 cleanup（清理结果），避免把主动停机当成 OOM。
检查：

- 插件及父级 CPU `nr_throttled`/`usage_usec` 增量与预期限额相符；父级 memory.max/CPU cap 不侵占实际内核/系统预留，kernel memory.min 至少 16 GiB。memory.min 是保护值，不是常驻内存或独占物理内存证明。
- OOM 后 `memory.events.oom_kill` 增长、无宿主或内核 OOM；PID 压力使 `pids.events.max` 增长。若实例重建导致计数器消失，保存 daemon 日志和重建前采样，不能把缺失计数当作通过。
- 另一插件仍可调用；管理 getNode 在压力阶段持续可用，报告最大/P95/P99 延迟及采样失败数。测试前约定管理延迟目标（建议先用 5 秒采样超时门槛）；本工具不是高频延迟基准。
- 在压力下实际执行一次读取、停用和重新启用管理操作，记录 operation 完成时间。只读 getNode 通过不能替代写管理可用性。

## 3. 并发调用与父级额度

使用 child account 的测试凭证与低并发配额，父级配额低于两个子账户配额之和；分别运行 calls 以观察共享父级限额。`invoke.json` 使用真实已安装 contract 和 provider_deployment_id，格式参考运行指南。禁止直接修改核心数据库构造“通过”的状态。

```bash
python3 tools/acceptance/run03.py calls --isolated-test-host \
  --config /etc/xelab/kernel.toml --daemon-pid "$RUN03_DAEMON_PID" \
  --token-file child-session.token --ca-file /etc/xelab/tls/server.crt \
  --request-file invoke.json --workers 16 --requests 1000 --seconds 120 \
  --output run03-evidence/calls-child-1.json
```

期望达到上限时明确拒绝，无超售；调用结束/故障回收后并发 used 与未释放 call reservation 的祖先合计一致。快速 echo 可能不能稳定触发并发饱和；此时本项必须保持未验证，使用经过签名和契约校验、支持有界慢调用的测试插件补测，不能把没有拒绝当作通过。

## 4. 插件故障、daemon 崩溃恢复、资源释放

1. 保留压力与请求证据，确认 `$RUN03_DAEMON_PID` 仍是上述正式 daemon（collector 会校验 exe/config/cgroup）。在授权 VM 手动对该 PID 注入 SIGKILL，记录时间。启动同一 SHA、同一配置的 xelabd 并更新 PID；不要同时运行两个控制器。
2. 用新的 collector 验证真实 daemon 重启。核对实例 ID、generation、reservation 唯一性，旧实例先停止，未知调用不重发、不伪造成功；ready 恢复后 API/CLI 可用。
3. 用公开 setDeploymentEnabled 停用全部测试部署并等待 operation 完成。保存 `getNode`、`getQuota`、`getInstance`、容器清单和最后 30 秒 collect，确认插件进程退出、cgroup 清除、节点资源账本归零、无未释放 call reservation/并发额度差异。若尚有非测试资源则不能要求归零，因此必须使用独占测试库/宿主。
4. 通过正常进程信号停止测试 daemon。工具不负责自动拆除主机网络、镜像或测试数据库；异常时先保存证据，再按运行指南人工恢复或销毁整个 VM。

## 证据与判定

输出包含 commit/二进制 SHA（commit 由执行人记录）、UTC 时间、实际资源、HTTP 状态/延迟、计数器与只读账本聚合；不包含令牌、密码、DSN、API 请求/响应正文。数据库采样用只读 repeatable-read 事务；失败保留类型并标记 collection_failed，不泄露连接字符串。保管 daemon 日志，上传前检查是否有操作人员额外打印的秘密。

| 项目 | 必需证据 | 初始状态 |
|---|---|---|
| 正式 daemon + 4C16G | 宿主说明、preflight、PID/config/binary hash、实际父级 cgroup | 待验证 |
| CPU 持续饱和 | 至少 10 分钟、所有压力实例及父级计数器增量 | 待验证 |
| OOM/PID 隔离 | 故障前后计数、另一插件和管理 API 正常 | 待验证 |
| 父级资源/并发 | 拒绝/等待超额、祖先配额和 reservation 一致 | 待验证 |
| 负载下管理 | 读/写操作、延迟分位数和错误数 | 待验证 |
| 插件/daemon 恢复 | 注入时间、旧进程围栏、同意图恢复、unknown 语义 | 待验证 |
| 全部释放 | 容器/cgroup 清理、零残留账本或逐项解释 | 待验证 |

退出码 0 只表示所请求的采集完成（preflight 为环境初步合格）；stress 的非零码需按证据区分预期终止与测试失败。所有 JSON 的 acceptance 保持 pending。只有上述全部项目有实际证据且审阅通过，才可在另一个验收记录 PR 中关闭 RUN-03。CI 的工具单元测试覆盖超预算、task/cgroup/代次不符、远端超时残留及清理失败。
OCI 步骤额外运行真实 exec 暂停/放行/清理组件测试；该组件明确替换 CI 无法满足的
4C16G/SQL 宿主准入观察，不是完整 RUN-03，不能充当真实宿主验收。
