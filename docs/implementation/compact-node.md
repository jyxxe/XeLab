# compact：1C512MiB 节点与整机验收

[文档总目录](../README.md) · [Issue #84](https://github.com/jyxxe/XeLab/issues/84) · [资源预算](../runtime/budgets.md)

状态：新增 opt-in 运行档位；**整机稳定性验收待执行**。编译、单测、Schema 校验和大机器上的小 cgroup 都不能证明 1 vCPU/512MiB 整机稳定。下文是可复现实验及通过条件，不是已完成的容量认证。

## 范围和预算

`resources.profile = "compact"` 面向单用户、小规模 API/CLI 控制面；旧配置省略 profile 仍为 `standard`，内核底线继续为 4 vCPU/16GiB。镜像在另一台机器构建 release 二进制，目标机仅运行发布产物。

| 项目 | compact 下限或边界 |
|---|---|
| 宿主 | 1 vCPU、标称 512MiB、无 swap；OS/containerd/PostgreSQL/日志全在本机计量 |
| 实测准入 | 1000mCPU，MemTotal 与祖先 cgroup 上限求交后至少 480MiB；最多32MiB启动保留不加回插件预算 |
| 内核预留 | 500mCPU / 201326592 bytes（192MiB） |
| 系统预留 | 250mCPU / 268435456 bytes（256MiB） |
| 插件池 | 实测容量减两项预留；有效512MiB时250m/64MiB，有效480MiB时250m/32MiB |

compact 内核 cgroup 同时设置 `memory.min`、`memory.max=kernel_memory_bytes`、`cpu.max=kernel_cpu_millis` 和 `memory.swap.max=0`，最低值对应192MiB/500m硬上限；standard保持原来的内存保护、不加这组硬上限。预留不等于内核RSS实测值。可提高预留，但剩余插件池会减少。宿主外的负载发生器和用户浏览器不计服务器内存；目标机内的采集器、SSH、数据库客户端及日志均计系统开销。

默认保持 headless，通过 HTTPS API/CLI 管理。现有 WebUI 建议512MiB，MPR父默认4vCPU/4GiB，本地 embedding/AI 有各自预算，均不属于最低机器基线；只有签名制品实际资源完全放入剩余池才可启用。不改写制品限制或允许借用内核预留。大镜像/归档备份与恢复也不属于最低工作集的稳定性结论：单任务元数据及上传缓冲仍有显著占用，应在实际数据规模下另测或扩大内核预留。

## 运行参数

| 入口/工作 | compact 限制 |
|---|---|
| HTTP | 两个共享响应生命周期槽；包含直接 HTTP、静态响应和 SSE，响应Body释放才退槽（包括SSE；仅读到EOF不提前退槽） |
| JSON/消息体 | 普通HTTP请求线缆正文≤256KiB（含chunked），仅raw `uploadBlob`流式上传豁免；业务/插件JSON输入输出及插件SSE帧/项在解析或收集前≤256KiB |
| RPC消息 | HostService服务端编码/解码及CapabilityService提供者响应解码≤320KiB（256KiB业务JSON加64KiB protobuf余量）；嵌套业务JSON仍≤256KiB |
| 活性探针 | `/health/live` 单独一个槽，业务/SSE 满载不借用此槽；readiness 仍检查真实状态 |
| HostService | 反序列化前到响应Body释放全程共享2槽（含所有连接/方法/流、EOF后仍持有），满时立即gRPC RESOURCE_EXHAUSTED；原handler/proxy2、Blob1、上传1限制另行生效 |
| PostgreSQL 客户端 | 每个内核进程最多16个连接；用尽后有界拒绝，不无限创建连接 |
| 后台 worker | 备份/恢复、升级、Schema初始化及备份上传清理共用1个进程级维护槽；各监督器最多1个在途job、空闲轮询1秒，任务及失败清理结束才退槽 |
| Tokio | 1个 worker thread，最多8个 blocking threads |
| 密码工作 | 所有 profile 全进程共1个昂贵密码操作；Argon2参数不变，阻塞计算退出前保留许可 |

raw Blob传输及静态非JSON资源继续遵循原字节/配额限制。管理元数据页和数据库聚合结果不等同业务JSON限额；大目录/列表、签名制品和大型归档仍须按实际数据规模另行验收。

维护槽只为备份/升级/Schema/清理四类监督器保留固定长度FIFO顺序，不为每个job创建等待task；轮到但不再轮询的类别2秒后让出。各持久队列用稳定UUID游标分页并环回，取得槽后才推进；反复失败的旧job不能持续遮住后续页。

profile在节点初始化时确定。已有node_budget与配置profile不一致时启动拒绝；不能仅改TOML再重启完成切换，应使用新节点或将来的显式迁移。在线资源预算更新也不能改变profile、重建线程池/连接数/入口槽。业务 overload 使用现有 RESOURCE_EXHAUSTED/服务不可用语义；槽有限不表示任意负载的延迟有保证。签名、TLS、权限、审计、cgroup、配额与幂等/unknown规则不因 compact 放宽。

## 配置与数据库

compact仍使用普通 `xelabd --config /etc/xelab/kernel.toml`，无需额外启动命令。第一阶段持有宿主排他租约，保存原有效宿主容量，设置内核cgroup硬限额并入组，然后只exec自身一次；同一租约文件描述符跨exec持有，验证固定文件身份/锁并恢复CLOEXEC，直到进程退出才释放。新进程映像在组内重新分配配置/Schema、Tokio与数据库状态。

第二阶段验证实际成员为`/xelab/kernel`，以原启动容量、物理内存、CPU affinity及当前`/xelab`祖先限制求交，只排除自身kernel叶子的内核限额。先使用重新读取的TOML启动限额，数据库恢复后preflight再应用已持久化的同profile预算。exec释放旧匿名内存，但共享ELF和文件缓存可能仍计于先前组；不能据此声称全部共享页均被重新计账，因此保留整机采样及RSS/HWM交叉检查。

复制 [compact配置](../contracts/examples/kernel-compact.toml)，填写实际 TLS、CA、数据库DSN、密钥、存储及网络。先运行 `xelabctl validate-config`，按[内核运行步骤](../../crates/kernel/README.md)显式迁移/引导，再启动 daemon。

独立 PostgreSQL 采用 [compact 起始配置](../contracts/examples/postgresql-compact.conf)：`max_connections=24`、`shared_buffers=16MB`、`work_mem=1MB`、`maintenance_work_mem=16MB`；保留超级用户恢复连接，并发 vacuum/parallel 降低。配置只是一组预算起点；work_mem按查询算子分配，不能把它乘连接数作为严格总内存上界。实际值及数据库RSS须纳入验收。依据 [PostgreSQL内存参数](https://www.postgresql.org/docs/16/runtime-config-resource.html)和[连接参数](https://www.postgresql.org/docs/16/runtime-config-connection.html)。

`fsync`、`synchronous_commit`、`full_page_writes` 保持开启，WAL及数据落持久盘，不关闭认证/审计或将临时盘当持久数据库。持久性依据 [PostgreSQL WAL说明](https://www.postgresql.org/docs/16/runtime-config-wal.html)。系统盘/XFS、日志轮转和备份空间仍需独立配置；1C512MiB不表示无磁盘容量要求。

## 独占测试机准备

1. 创建独占 Linux VM，hypervisor 配置恰好1vCPU/512MiB，禁用内存热加及 balloon 扩容，无 swap/zram。保存配置导出；容器内512MiB配额不等价整机VM。
2. 安装 containerd/runc、cgroups v2、网络工具、PostgreSQL、XFS配额及 release xelabd；记录OS/内核/依赖版本、commit SHA和二进制摘要。最低档不在目标机编译。
3. 配置 compact、本地数据库和只用于本次实验的凭证。记录 `getNode` 的实测CPU/内存、预留与剩余池。不能手工写入合成大容量 node_budget。
4. 确认 `MemTotal >= 491520 kB`、SwapTotal=0、在线CPU=1；核验内核 `memory.min`、插件 `cpu.max/memory.max/memory.swap.max` 与真实预算。导出 PostgreSQL `pg_settings` 及 `pg_stat_activity`，不能导出DSN或明文令牌。
5. 负载发生器在另一台机器，使用受限测试API Key、可信CA，保留状态码/延迟/操作ID而不保存密码、Token或业务正文。

```bash
uname -a
lscpu
cat /proc/meminfo
cat /proc/swaps
cat /sys/fs/cgroup/xelab/kernel/memory.min
cat /sys/fs/cgroup/xelab/plugins/{cpu.max,memory.max,memory.swap.max}
sha256sum /usr/local/bin/xelabd
ps -C xelabd,postgres,containerd -o pid,comm,rss,etimes
# 在本机经管理员已配置的安全连接执行，不把DSN写入命令行。
psql -X -c "SELECT name, setting, unit FROM pg_settings WHERE name IN ('max_connections','shared_buffers','work_mem','maintenance_work_mem','fsync','synchronous_commit','full_page_writes');"
psql -X -c "SELECT state, count(*) FROM pg_stat_activity GROUP BY state;"
```

## 24小时空闲与1小时轻负载

只读 [compact_node.py](../../tools/acceptance/compact_node.py) 按实际PID、启动时间、cgroup和配置路径锁定 daemon，记录主机MemAvailable、全局OOM、PSI、内核/插件cgroup内存/事件，以及daemon的`/proc/<PID>/status`中VmRSS/VmHWM/VmSwap；JSONL逐条写入，避免全天样本留在RAM。RSS或自进程启动以来的HWM超过内核预算、或VmSwap非零均失败，防止仅观察cgroup时遗漏入组前分配；摘要单列RSS/HWM峰值。它不产生业务流量，不替代API可用性、数据库持久性或hypervisor证据，所有报告的 `acceptance` 始终为 pending。

在线 CPU 数从 Linux `/sys/devices/system/cpu/online` 读取，不受 Python CPU 数量覆盖设置影响；该文件不可读或格式异常时采集失败。此检查仍需结合 hypervisor 证据确认整机形态。

在VM上用两个不同输出文件分阶段采集（`<PID>` 为已核实 daemon）：

```bash
python3 tools/acceptance/compact_node.py --config /etc/xelab/kernel.toml \
  --daemon-pid <PID> --whole-node-vm --seconds 86400 --interval 10 \
  --output idle-24h.jsonl
python3 tools/acceptance/compact_node.py --config /etc/xelab/kernel.toml \
  --daemon-pid <PID> --whole-node-vm --seconds 3600 --interval 5 \
  --output light-1h.jsonl
```

空闲阶段每30秒从外部检查 `/health/live`、`/health/ready` 和 `getNode`。轻负载阶段持续1小时，以1请求/秒交替 `getNode`、`getQuota`、列表查询；每60秒完成一次登录/登出，以单个并发执行安全的管理写入并核验持久结果。记录实际请求数、成功率、p50/p95/p99与最大延迟，正常请求目标100%成功、p95≤2秒、最大≤5秒；这些是待验证门槛，不是预先保证。

每10秒另采集数据库连接数与PostgreSQL/containerd RSS，保留查询器本身的连接/内存开销。

若真实剩余池可放下已签名轻量插件，再执行单并发、≤1KiB输入输出、1次/秒能力调用，并记录镜像摘要、签名资源、限额和成功率。不能缩小现有制品声明来完成测试；插件无法准入则记录 RESOURCE_EXHAUSTED，基础 headless 结果与带插件结果分别呈现。

## 突发、长流、恢复

- 业务突发：外部8并发持续60秒，混合小读取/登录/已允许调用；保留过载拒绝而不是重试掩盖。正常轻负载停止后≤5秒恢复管理读取；`/health/live` 单并发每秒探测应持续成功。
- 慢流：开启2条获准SSE并慢读60秒，第三条应有界拒绝；关闭一条后新请求可用。同时检查活性专用槽、断开后的物理退槽、数据库连接和内存回落。
- 内存/CPU边界：只在已有签名插件限额内发送合法压力，不故意使内核或PostgreSQL OOM。启用超预算制品应拒绝/等待并保持账本一致。
- 重启：停止负载后保存读写/审计证据；分别正常重启daemon、SIGKILL daemon后重启、整机重启，各重复3次。每次用新PID开启新采集文件，验证≤60秒恢复Ready，已确认写入与审计仍在，无重复实例/预留；副作用不盲重派。
- 恢复后重复5分钟轻负载，停止测试部署并核对容器退出、插件占用和调用预留释放。数据库损坏、失去已确认写入、孤儿进程或自动无限重启均失败。

采集器在PID退出/复用时失败是预期证据；不能把该文件改成连续成功。重启时段由新旧文件、boot_id、journal和独立探针时间线拼接核实。

## 通过条件和证据归档

保存VM配置、commit/binary SHA、脱敏配置和数据库参数、24h/1h完整JSONL、外部请求摘要、重启时间线、`journalctl -k` OOM记录、数据库恢复日志及前后资源账本。必须同时满足：

- 目标整机1vCPU/512MiB且无swap，所有服务在机内；采集完整、最大采样间隔符合报告，实测容量没有被合成配置替代。
- daemon当前RSS及启动以来HWM均不超过内核预算，VmSwap=0；正常空闲/轻载/突发阶段全局及内核/插件 OOM、OOM-kill、非预期进程退出增量均为0；内存不随时间单调增长至耗尽，空闲结束与负载后回落有可解释余量。
- 正常API延迟/成功率达到上述门槛，过载有界拒绝，活性与负载后恢复满足要求；1小时数据库连接始终≤客户端/服务端限制。
- 重启后持久数据、权限、审计及资源账本一致；原standard测试保持通过。最小档验收不能关闭standard的 [RUN-03](run03-host.md) 或独立灾备认证。

未收集或未审阅上述证据前，Issue #84 的状态只能为“实现已提供，1C512MiB稳定性待验收”。
