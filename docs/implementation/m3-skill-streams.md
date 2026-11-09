# M3 普通 OCI Skill 流式调用


> 2026-11-09 验收补记：M3 功能提交 #45/#46/#47 的精确完整与专项 CI 已全部通过。下文保留相应切片开发时的命令、历史状态和证据边界；当前验收表与精确 head/tree/CI 见 [当前验收记录](m3-acceptance-20261109.md)，最终集成与 main CI 见 [目标 #38](https://github.com/jyxxe/XeLab/issues/38)。历史“待验收”表述不得替代该最新记录，组件通过也不替代真实 OCI 或正式生产验收。
[实施索引](README.md) · [Skill 注册与非流式调用](m3-skills.md) · [持续目标](m3-progress.md)

本页记录 `streamSkill` 的实现切片及验收入口，不代表整个 M3 已完成。
实现使用现有 OpenAPI、`SkillStreamEvent`、`StreamEvent` 与 CapabilityService Proto，
不增加并行授权系统或数据库迁移。

## 调用与身份

`POST /api/platform/v1/skills/{skill_id}/calls:stream` 接受既有 `SkillInvokeRequest`，
要求 `Idempotency-Key` 和有效身份；Cookie 来源及 Origin 检查与管理 API 保持一致。
Skill 必须声明与签名 capability 精确一致的 `stream_item_schema`。
普通 capability 的 none/idempotent/queryable/non_repeatable 均可声明流；
副作用不同不改变固定绑定、终帧验证或禁止自动重开的规则。公开版本化数据制品仍关闭。

流和非流式 Skill 共用 Broker 的准入事务、固定安装/契约选择、
`skill.invoke` 与目标 `capability.invoke` 双层授权、原始来源计费、账户额度、
实例共享并发池、RPC 委托及最终 Call 账本。
外部 JSON 无法提供内部 Skill 上下文或任意实例地址。
派发前、在途、结果提交前和每帧交付前重新检查当前身份与原始已计费来源；
未参与该 Call 计费的新来源不能维持旧调用。

## 真实 RPC 与 SSE

Broker 调用真实插件的 `InvokeStream`，逐块验证 call ID、从 1 开始的连续序号、
严格 JSON、固定 capability/Skill 的 item Schema，以及最终 output Schema 和摘要。
缺少完成结果、完成后出现其他块、重复或跳跃序号、错误 call ID/摘要均失败。
不把非流式结果拆开伪装成流，也不在内存中收集整个输出再发送。

持久准入提交后才发送 `accepted`；准入失败保留 JSON HTTP 错误状态。
业务 SSE `data` 是 `SkillStreamEvent`，包含固定 UUID、版本及 definition_digest；
内部 `event` 为既有 `StreamEvent`。`completed.payload` 是 `CallResult`，
其内也保存 Skill UUID 与定义摘要，便于 `getCall` 核实历史来源。
`event:` 与 kind 一致，`id:` 是 `call_id:sequence`；accepted 从 1 开始，
随后零到多 item，最后一个 completed/failed/canceled，终帧后不能继续输出。

生成流拒绝 `Last-Event-ID`；同一 HTTP 幂等键不重新生成、不重放旧流，
返回带原 Call ID 的冲突供调用者查询。断线后不得自动重试并拼接输出。
HTTP 200 之后的错误使用终帧；若连接已断开或消费者不再推进，
终帧无法保证送达，客户端必须通过 `getCall` 判断结果。

## 有界资源与取消

单个序列化业务事件（包括 Skill 信封及 completed 结果）上限 1 MiB，
item 必须为信封留出空间；capability 的更大 maximum_output_bytes 不能放宽流的单块限制。
队列容量为 2；连同生产者及消费者各一块，
至多四块业务缓冲，低于契约的 16 块上限。RPC 解码另设单块上限。
进程内另设 64 个保留 SSE 响应的执行 profile 上限，属于内部资源限制，
不增加公开字段。准入前非阻塞取得许可；许可随实际 HTTP Body 持有，
生产者结束、物理 Call 预约回收或读取到 EOF 都不提前归还，Body 丢弃后才释放。
达到上限返回 RESOURCE_EXHAUSTED，不能继续创建 Call 或扣账。
事件校验或发送失败不会消耗未交付的序号。
队列满时暂停读取插件 RPC；消费者 30 秒不推进或整体 deadline 先到则终止。
首次慢消费者超时后，终帧只尝试一次非阻塞发送，不再次等待 30 秒；
缓冲仍满时关闭生产者，客户端按既有契约查询 getCall。
交付鉴权的数据库等待也受原始 deadline 限制，不能靠持续心跳延长业务调用。
取消敏感查询使用请求作用域连接：丢弃时向准确的 PostgreSQL backend 发送取消，
有界清理后关闭驱动，并保留服务端 statement_timeout 兜底。
HTTP `Cache-Control` 为 `no-store`，15 秒注释心跳不占业务序号。

断线、撤权、禁用 Skill/部署、明确 `cancelCall` 或 deadline 会停止后续交付、
丢弃 RPC 流并撤销委托。参考插件检查接收端关闭、取消、排空和 deadline。
客户端在准入锁等待时断线会立即丢弃准入任务，事务回滚且不派发 RPC。
若断线恰逢数据库提交，可能留下已持久准入而尚未派发的 Call；
派发前再次检查接收端，保留该不确定记录并按原 deadline 与控制器围栏回收。
`cancelCall` 记录取消意图并返回 204；已派发调用保持 `unknown`，
错误标记为 `CANCELED`，不会谎称已撤销实际副作用。
非可重复或仍未知的活动预约由既有 Controller 在确认实例进程停止后释放；
queryable 已由原实例 QueryResult 确认的结果可在原冻结授权仍有效时结算。
未知结果不能触发自动重新派发；纯流同样保留这一恢复围栏。

## 参考插件与可复现测试

Rust `xelab-echo` 的签名清单同时提供原有 echo 和 `org.xelab.echo.stream@1.0.0`，
后者输入 message/count/delay_ms，输出 item(index/message) 及最终 message。
另有 idempotent/queryable/non_repeatable 三个固定契约及 Skill；后两类真实增加实例内计数，
其 after-commit 故障在副作用和结果记录已提交后中断传输。idempotent 参考操作是
向 SHA-256(message) 固定单元写入相同值，不使用递增计数；重新赋值的结果本身幂等。
结果缓存是实例内有界记录，
不能据此声称插件跨实例数据持久性或生产数据格式验收；缺失记录只能报告 unknown。
测试故障参数用于验证错误序号、call ID、item、output、摘要、缺失终块和终块后数据。
Python 和格式迁移参考制品保持原契约，不自动获得流式入口。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p xelab-kernel --lib skill_streams::tests --locked -- --nocapture
cargo test -p xelab-kernel --lib skills::tests --locked -- --nocapture
cargo test -p xelab-kernel --test skills --locked -- --nocapture
python3 docs/tools/validate_docs.py --strict
```

PostgreSQL 测试需要独立的 `XELAB_TEST_DATABASE_URL`。
元数据 API 测试覆盖非流式 Skill 拒绝流调用、Last-Event-ID、非法查询、
无提供者不扣账、取消权限及跨账户拒绝、等待准入时断线，
并在锁仍由其他事务持有时核实被取消的 backend 已退出，不据此声称运行了 OCI。
传输单元测试实际填满有界队列、验证消费者推进、终态和超大帧不跳号。
另通过真实 PostgreSQL 终止 leader 连接，验证已排队业务帧在就绪撤销、
停止或失去 leadership 后转换为无业务载荷的失败终帧。

已有 [Kernel M1 工作流](../../.github/workflows/kernel.yml) 构建签名 Rust OCI 参考制品，
真实 `signed_oci_plugin_runs_through_the_management_api_and_recovers` 场景执行：
注册启用、实际 item/完成结果及来源、重复请求拒绝、协议错误的真实实例围栏重启、
明确取消、API Key 撤销后停止交付、恢复后新调用，以及原有非流式和离线恢复回归。
故障扫查同时保留实际 OCI PID/启动时间的退出证据；现有重启预算耗尽时，
先验证拒绝新调用且不计费，再通过公开停用/启用创建新实例与 generation。
测试不直接清零计数或提高预算，也不把预约释放与重新 Ready 混为同一断言。
该入口须在独占 cgroups v2、containerd/runc、网络及 XFS 环境实际运行并保存 CI 日志。
本地无这些宿主权限时只记编译及已执行的 PostgreSQL/单元证据，真实 OCI 保持待验收。

## 尚未交付

`HostService.InvokeSkillStream` 等插件侧 Skill 方法仍明确 UNIMPLEMENTED；
HTTP Skill、MPR、Extension、Workflow、BatchEngine、OAuth、WebUI 和 UI 桥不属于本切片。
AWS 与独立宿主正式验收继续按[持续目标](m3-progress.md)标为 DEFERRED，
不降低备份认证、4C16G 预留或公开制品门禁。
