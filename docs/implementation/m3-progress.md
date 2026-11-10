# M3 实现目标与验收进度

本页保留的4C16G容量要求及既有证据属于 `standard` profile；[compact 1C512MiB](compact-node.md) 有独立准入与整机验收，不复用本页容量结论。

[实施索引](README.md) · [目标 Issue #38](https://github.com/jyxxe/XeLab/issues/38) · [里程碑](roadmap.md)

## 当前验收记录

2026-11-09（北京时间）已复核三个精确功能提交的完整/专项 CI 均成功。阶段、head/tree/CI、安全修复、生产边界和后续依赖见[当前验收记录](m3-acceptance-20261109.md)；最终集成和 main CI 以[目标 #38](https://github.com/jyxxe/XeLab/issues/38)实际结果为准。

## 2026-11-04 开发历史（保留当时状态）

以下状态快照截至 2026-11-04 23:07 UTC；其中未完成/待发布表述是当时事实，不代表上方当前精确提交状态。
GitHub 状态会继续变化，最终完成清单以[目标 #38](https://github.com/jyxxe/XeLab/issues/38)持续维护的实际证据为准。
初始基线为 main
`dbb35cbc7c2c110c88de7ba16dc2d4569fad3ad2`，内部 Schema 14。
这是开发与证据清单；勾选只表示该子项已有相应证据，不代表整个 M3-A/B 或 M3 完成。
后续阶段须更新实际提交、PR、测试命令及结果，不能用本页代替运行证据。
当前 main 为 `e9ad62b83708e46569ca47f74413f6890950cdb5`，内部 Schema 16：
[PR #39](https://github.com/jyxxe/XeLab/pull/39)、
[PR #40](https://github.com/jyxxe/XeLab/pull/40)、
[PR #41](https://github.com/jyxxe/XeLab/pull/41)、
[PR #42](https://github.com/jyxxe/XeLab/pull/42)、
[HTTP #43](https://github.com/jyxxe/XeLab/pull/43) 和
[OAuth/UI 控制面 #44](https://github.com/jyxxe/XeLab/pull/44) 已合并。
#43/#44 精确提交 CI 成功；main 自身 [CI 41860893005](https://github.com/jyxxe/XeLab/actions/runs/41860893005) 也已全部成功，包含 242 默认 Rust、真实 OAuth 浏览器 42 checks、OCI/XFS/恢复及严格文档检查。
已合并的七项 Skill 操作、首个普通纯调用/流 profile、HTTP 基础及 OAuth 控制面不能代替完整目标。
[Draft #45](https://github.com/jyxxe/XeLab/pull/45) 的独立 WebUI/桥、
[#46](https://github.com/jyxxe/XeLab/pull/46) 的普通四类副作用/QueryResult 和
[Draft #47](https://github.com/jyxxe/XeLab/pull/47) 的 HTTP Skill 六方法/SSE 均已有源码，
最终合并及 main 门禁未完成；三个最新 focus 均已通过，full #45 发生登录前静态资源504，full #47 发生崩溃恢复夹具的后台锁冲突，#46 full/focus 已全部通过。新发现的两项 UI 元数据泄露已在本地实现修复并完成定向测试，最终树尚未验收，M3 仍未完成。#42 的严格容量诊断与精确 tar 清理保持，
其成功不能倒推旧容量失败的根因已被证明。

## 范围与协作

维护者已授权按最小可验收功能开发、提交 PR，在最终 CI 和审查门槛满足后合并。
主执行负责人为当前 Codex；GitHub 负责账号为 `jyxxe`，任务追踪见 Issue #38。
开始分支为 `feat/m3-skill-registry-router`；协作者仅处理已划定范围。
先完成正确性、授权和安全边界，再完成实际闭环、回归恢复及 UI 体验。

保留 Rust 内核、PostgreSQL、containerd/runc、cgroups v2、签名 OCI 和 mTLS。
复用 Broker、Runtime、Controller、Identity 与 Authorizer，追加迁移且不改历史校验和。
保持 4C16G 内核预留、现有来源链/幂等/审计/配额语义和 fail-closed 门禁。
不扩大到 MPR、Extension、Workflow、BatchEngine 或完整 M4–M6；
未来操作明确不可用，不生成虚假成功。XeLibrary 不属于本项目实施依赖。

## 初始源码差距

以下核对针对上述基线，不能作为后续提交已经完成的结论：

| 领域 | 实际基线 | M3 必需结果 |
|---|---|---|
| Skill | 无 Skill 注册/路由模块；HostService Skill 方法明确 unimplemented | 固定安装/版本/摘要，注册管理、双重授权及真实调用/流 |
| Broker | 已有直接非流式调用、共享实例并发、来源计费和 unknown 账本 | Skill 路径复用这些边界，传播 deadline/取消/撤权 |
| HTTP | 清单准入拒绝非空 routes 和 UI；无声明式网关 | 精确签名路由、业务授权、受控静态 GET/HEAD、SSE |
| 认证 | 已有账户、密码、API Key、会话及实时来源授权 | 可信 code/PKCE、Client 管理、同意页、轮换及重用检测 |
| WebUI | 无独立官方 OCI WebUI 或实际浏览器验收 | 公开 API 管理、桌面窗口主题、独立部署及停用恢复 |
| UI 桥 | UI Contribution/xelab.ui/1 为契约 | 固定贡献、隔离 iframe、绑定 Skill 调用及通道失效 |

依据为 `crates/kernel/src/{lib,artifact,broker,host,api,identity}.rs`、
[内核实现说明](../../crates/kernel/README.md)、[Skill 验收](../skills/acceptance.md)和
[WebUI 验收](../preset-plugins/webui/acceptance.md)。
现行机器契约是目标；契约冲突须最小一致修正，不删除限制。

## 分阶段清单

- [x] Phase 0：重新读取 main、提交、开放 PR 和现行规范；#1 规划 PR 保持开放。
- [x] Phase 0：审查并合并 [PR #31](https://github.com/jyxxe/XeLab/pull/31)，建立 Issue 入口。
- [x] Phase 0：建立 [M3 目标 Issue #38](https://github.com/jyxxe/XeLab/issues/38)，搜索时无已有 Issue。
- [x] Phase 0：完成适用 M1/M2 基线及真实组件 CI 回归，修复阻断开发的问题；本地宿主差异和正式验收边界见证据。
- [x] M3-A：registerSkill、getSkill、listSkills、setSkillEnabled、archiveSkill；已随 #39 合并。
- [x] M3-A：invokeSkill 经 SkillRouter → Broker → 真实固定 OCI 实例；已随 #39 合并。
- [x] M3-A：streamSkill 经相同 Broker 与固定真实 OCI 实例；普通 capability 纯流 profile 已随 #40 合并。
- [x] M3-A 首个普通纯 profile：签名/安装/Schema/版本冲突、双层授权、撤权取消及共享并发；见 #39/#40 精确 CI。
- [x] M3-A 首个 profile：独立插件注册→启用→授权→调用→禁用拒绝→恢复，含重复请求、资源不足与实例重启；不是全部副作用验收。
- [ ] M3-A 完整普通 profile：none/idempotent/queryable/non_repeatable 的单次与已声明流、原提供者 QueryResult 已实现，#46 真实 OCI 子场景已通过，最终整轮 CI 与合并待完成；HostService Skill 代理属 M4，不冒充已实现。
- [x] M3-B 基础：精确签名 HTTP/OpenAPI、动作/Schema、首个 HTTP Skill、隔离静态 GET/HEAD、共享物理池、直接 HTTP SSE、超时/撤权/恢复及独立 HTTP-only/混合插件；#43 已合并且精确 CI 成功。
- [ ] M3-B 完整 HTTP Skill：固定 GET/HEAD/POST/PUT/PATCH/DELETE、声明成功状态/null identity 和 streamSkill SSE 已在 #47 实现，真实 OCI 子场景已通过，最终整轮 CI 与合并待完成。
- [x] M3-C 控制面：Client 注册/查询/修改/禁用、精确 origin/redirect/scope、默认 UI；#44 已合并，精确 CI 成功。
- [x] M3-C 控制面：可信登录/原生同意、code + PKCE S256、单次码、刷新轮换/重用撤族、会话/主体/Client/Origin/CSRF/Cookie 及拒绝测试；实际可信页面浏览器已运行，不是 OCI WebUI 全链。
- [ ] M3-D：#45 已实现独立 OCI `org.xelab.webui` 的签名构建、配置、限额、部署与访问步骤；整体门禁未通过。
- [ ] M3-D：桌面、Dock、启动器、多窗口、组件、主题、小屏与键盘操作已由组件验证，真实核心/OCI 管理验收仍待 #45。
- [ ] M3-D：管理 API/OAuth 代码及真实结果/错误反馈存在，不能以组件夹具结果替代真实核心管理。
- [ ] M3-E：签名 window/widget 发现与固定安装/部署/版本/摘要/alias 已实现，#45 整体待验收。
- [ ] M3-E：iframe、view/nonce/parent_origin、source/origin/protocol/seq 校验及负向组件已实现，真实独立 OCI 证据待完成。
- [ ] M3-E：skill.call、同部署 window.open、撤权/登出/升级失效和跨账户组件已实现；#45 真实完整路径未通过。
- [ ] M3-F：可信 Owner→真实插件→Skill→OAuth→WebUI→iframe→桥调用完整实际端到端。
- [ ] M3-F：撤权拒绝、停用 WebUI 后 CLI 可用、重启后持久状态/配额/边界一致。
- [ ] M3-F：自动化浏览器测试、Rust/数据库/契约/fmt/Clippy/workspace 和故障回归证据。
- [ ] M3-F：必需 PR 均合并，main 最终 CI 成功，功能/接口/迁移/文档一致。

## 已执行证据与待验收项

#43/#44 已分别在精确 CI 成功后合并。#43 的恢复测试旧 Host 已按各自实际 Config 修正；
生产 Host 校验保持。#44 旧 CI 中的私有恢复 404、ENOSPC 和未执行项保留历史，
新通过不把旧失败改写成成功。各阶段 PR 的快照如下，运行/排队均不算通过：

| PR / 精确 head | CI 快照与尚缺结果 |
|---|---|
| [#43](https://github.com/jyxxe/XeLab/pull/43) `68522810…` | [41850322061](https://github.com/jyxxe/XeLab/actions/runs/41850322061) 成功；已合并为 `a285c4ba…` |
| [#44](https://github.com/jyxxe/XeLab/pull/44) `f427b4c5…` | [41851424259](https://github.com/jyxxe/XeLab/actions/runs/41851424259) 成功；已合并为 main `e9ad62b8…` |
| [Draft #45](https://github.com/jyxxe/XeLab/pull/45) `3a3e0479…` | [41922158262](https://github.com/jyxxe/XeLab/actions/runs/41922158262) focus 成功：53 browser+11 Core、5 unique/6 POST、隐私flags=false/errors={}。full 的 api.js 返回504，可信登录和Skill调用尚未发生；观察到 static_unknown=1 的物理围栏，具体未完成阶段待固定枚举诊断，不能称整体通过 |
| [#46](https://github.com/jyxxe/XeLab/pull/46) `7249b969…` | [41922164310](https://github.com/jyxxe/XeLab/actions/runs/41922164310) full/focus 全部成功：均为53 browser+11 Core，workspace/docs/protocol/lints/tracked-clean全部成功。历史 `4e8a42c2…` 的 [41906679990](https://github.com/jyxxe/XeLab/actions/runs/41906679990) 和 `0306352c…` 的 [41918457925](https://github.com/jyxxe/XeLab/actions/runs/41918457925) 也曾 full/focus 全绿；这些不替代新隐私修复验证。已 ready，依赖 #45，未合并 |
| [Draft #47](https://github.com/jyxxe/XeLab/pull/47) `0cd93c72…` | [41922172107](https://github.com/jyxxe/XeLab/actions/runs/41922172107) focus 成功：53 browser+11 Core、5 unique/6 POST。full 的真实HTTP/ordinary通过，随后 XFS 进程崩溃夹具在 initializing 阶段收到后台锁冲突；未到真实崩溃检查点，F/workspace/docs/protocol未执行。测试观察竞态修复待新真实CI |

本地尚未发布的稳定化包括公开 CSP 与 UI 游标的两个 P1 元数据泄露修复：
CSP 不再枚举其他账户的部署来源，非默认动态壳使用仅限 framing 的固定政策证明；
游标使用目的隔离 AES-GCM，并在 LIMIT 前按现 Policy 和实际 owner 预选。
固定 ID 与 owned_by 的交集由可选只读候选收集器补全，再以原 Policy 逐对核实，
不改变授权结果或把 owner 限定权限扩大到全局 ID。
4项新增真实 PostgreSQL/独立 mTLS 回归已通过（80.88秒），覆盖非默认壳、
Client/审批变化、query/凭证不转发、208个私有贡献不产生数量桶、合法翻页、
跨来源游标拒绝及合法共享/错误 owner；这不是最终树全回归或真实 OCI 验收。

前端严格 framing 参数及非默认壳实际 F 场景已提交 `d3515a9…`；
XFS 夹具观察竞态修复已提交 `3f2be5b…`。后者仅把启动轮询改为逐层
O_NOFOLLOW 的无锁只读观察，实际标记出现后仍执行原完整 XFS 校验并比对 inode/检查点，
没有修改生产存储锁或放宽崩溃恢复断言。临时文件 flock 对照40轮通过，
旧现场的实际 errno/持锁者仍未知；必须由新真实 XFS CI 验证。


目录缺口与实际网络证据分别记录：早期缺少列表实现的503是源码分析，
当时真实浏览器在带查询 OPTIONS400 停止，尚未发送目录 GET。
现四个列表接口与严格 preflight 顺序均已修复，最新真实 F 已观察目录 GET200。
窗口竞态由延迟偏好加载的 Chromium 回归确定复现；离线竞争由源码和独立审查确认可达，
旧失败日志未带实例 ID，不能将其称为唯一已证明的历史根因。

CI DWARF/incremental 产物优化和 90 分钟总作业预算只改变测试构建占用/作业预算，
不修改业务 deadline、并发、资源限额或安全断言。

官方独立 OCI WebUI、第三方 UI 参考插件和宿主消息桥已实现于集成分支，
Node 21、WebUI mTLS 9、reference HTTP 7、Chromium 桌面 14/桥 8/reference 4
项组件测试已执行通过；独立打包 Docker 探针也通过。rootless Docker 的 CgroupDriver=none，不证明实际逐容器资源限制。
真实签名两镜像、公开安装/Client 引导、Owner/子账户 OAuth、iframe Skill、攻击账本、
撤权、停止 WebUI 后 CLI 及同进程核心组件重启的 `m3_ui` 测试已接入 CI，并在 #46 上述历史精确提交两个 job 实际通过；当前依赖及后续修改仍须完整验证。
[实现与复现入口](m3-webui.md)明确组件、真实 OCI 与生产验收的证据边界。
普通副作用/QueryResult 和 HTTP Skill 六方法/SSE 的定向 PostgreSQL/mTLS 组件及合并源码检查已有本地结果，
详见 [Skill](m3-skills.md)、[流](m3-skill-streams.md)和[HTTP](m3-http.md)；组件与已通过的真实子门禁均不能替代 #46/#47 最终完整 CI。


| 证据 | 结果及边界 |
|---|---|
| PR #31 exact head `70a8f128426c50091aaa117a961d652449129733` | [Issue checks](https://github.com/jyxxe/XeLab/actions/runs/41750527904) 与 [Kernel M1](https://github.com/jyxxe/XeLab/actions/runs/41750527932) 成功 |
| #31 与 #37 的本地合并树 | 配置校验 5 表单/22 标签；10 单元测试；strict docs 812 页；diff 检查均通过 |
| main `dbb35cbc…` 标签同步 | [Sync issue labels](https://github.com/jyxxe/XeLab/actions/runs/41796871248) 成功；已读取并核对 22 个受管标签 |
| main `dbb35cbc…` Issue 配置 | [Issue checks](https://github.com/jyxxe/XeLab/actions/runs/41796871110) 成功 |
| main `dbb35cbc…` 内核回归 | [Kernel M1](https://github.com/jyxxe/XeLab/actions/runs/41796871078) 成功；正式生产/AWS 验收仍独立待执行 |
| M3-A 非流式实现 `7ab2f0e…` | [PR #39](https://github.com/jyxxe/XeLab/pull/39) 已合并；精确提交的 [Kernel M1](https://github.com/jyxxe/XeLab/actions/runs/41804075279) 成功，包含真实 OCI、离线恢复与适用回归；[范围和命令](m3-skills.md) |
| main `136cb02…` 合并后 CI | [Kernel M1](https://github.com/jyxxe/XeLab/actions/runs/41810812489) 成功 |
| 流式分支 `accc858…` 首次真实 OCI | [执行 41804847274](https://github.com/jyxxe/XeLab/actions/runs/41804847274) 失败：独立畸形流连续触发隔离，达到既有 5 次启动预算；测试已改为验证预算拒绝后通过公开停启恢复，不提高预算 |
| 流式分支 `8c1c915…` 真实 OCI | [执行 41809262982](https://github.com/jyxxe/XeLab/actions/runs/41809262982) 中流式正常/畸形/取消/撤权/共享并发/断线及预算恢复场景通过；随后升级测试按单能力计数失败，整体 CI 仍失败，不能计为完整通过 |
| M3-A 流式实现 `e95e584…` | [PR #40](https://github.com/jyxxe/XeLab/pull/40) 已合并；精确提交的 [Kernel M1](https://github.com/jyxxe/XeLab/actions/runs/41813120114) 成功，包含实际 OCI、恢复和适用回归 |
| main `dca7c8d…` 合并后 CI | [执行 41816557404](https://github.com/jyxxe/XeLab/actions/runs/41816557404) 失败：live-revoke 场景暴露 Broker 定时复核与执行未来之间的授权锁活性问题；90 秒夹具亦复现，不能作为单纯测试超时处理；已由 #41 修复并合并，失败历史仍保留 |
| Draft #41 `1399b619…` | [执行 41819988198](https://github.com/jyxxe/XeLab/actions/runs/41819988198) 在 image_archive 的宿主 I/O 阶段失败，尚未执行到 OCI/撤权回归；追加诊断不改变验证/限制，不能记为修复通过 |
| Draft #41 `3ed37c67…` | [执行 41823170948](https://github.com/jyxxe/XeLab/actions/runs/41823170948) 实际 OCI 再现撤权等待到 deadline：执行未来持有共享授权事务时，监视分支等待排在独占撤权写入者后的新共享事务，却暂停轮询执行未来；并发 guard 修复及真实 PostgreSQL 屏障回归尚在验证 |
| #41 `20e09827…` | 独立并发 guard 修复及实际 PostgreSQL/OCI 撤权回归通过；[执行 41827862054](https://github.com/jyxxe/XeLab/actions/runs/41827862054) 全步骤成功，PR 已合并；OCI 撤权约 739 ms，仍余 89261 ms，精确双事件 failed/INVALID_CREDENTIAL 和物理 fencing/配额均保留 |
| main `41e495bf…` | [执行 41832456991](https://github.com/jyxxe/XeLab/actions/runs/41832456991) 失败：实际归档恢复通过，OCI 离线捕获返回 RESOURCE_EXHAUSTED；已修撤权仍及时完成，容量来源及已消费构建 tar 清理修正独立处理，不盲目重跑或放松门槛 |
| #42 `534d0a1b…` | [P0 捕获容量修正](https://github.com/jyxxe/XeLab/pull/42) 已合并 main，精确提交[执行 41837147021](https://github.com/jyxxe/XeLab/actions/runs/41837147021) 全步骤成功。保留容量诊断与门槛，只在全部 immutable refs 核实后删除已消费的精确传输 tar；HTTP 分支增加两份 HTTP 制品且逐层拒绝符号链接。旧失败缺少实际容量数值，不能把此次成功当作已证明旧容量根因 |
| main `201fb5a…` | [执行 41845259790](https://github.com/jyxxe/XeLab/actions/runs/41845259790) 已独立成功；不包含本次 #45–47 的新增范围 |
| Draft #43 `c537bef…` | [执行 41841936407](https://github.com/jyxxe/XeLab/actions/runs/41841936407) 的实际 HTTP/static OCI、旧 OCI 生命周期、XFS 卷与进程恢复场景通过；随后平台恢复业务夹具用示例 Host 请求已改域名的内核，Owner 登录正确被拒 404，整体 CI 失败。夹具改为实际恢复配置 Host，生产隔离检查及恢复断言不变，新提交仍须完整验证 |
| HTTP 分支本地完整 workspace 尝试 | 库 150 通过/5 失败/3 显式忽略；1 项空 ContractRef 旧夹具按契约修正后通过；4 项归档/克隆因 overlay 目录属性列表长度/读取不一致拒绝，相同源码和检查在真实 tmpfs 隔离组件归档 2 项、克隆 3 项全部通过。保留原宿主失败，不冒充整轮 workspace 通过 |
| HTTP 后续安全/兼容回归 | API→JSONB 1.0 额度/轮换、首帧 SSE 拒绝的实际 mTLS 组件 2 项通过；结果读取/重放的原始计费与 Blob ACL PostgreSQL 3 项、Identity 15/Skill 8/PostgreSQL 2 项通过；基础 HTTP 后由 #43 最终 CI 验证，新增 #47 范围另待执行 |
| HTTP 基础开发历史 | [实现及命令](m3-http.md)、[持久化](m3-http-provenance.md)与[参考制品](../../examples/http/README.md)已存在；5 项契约 PostgreSQL、12 项 Python 及两个实际打包探针通过；核心/PG/独立 mTLS 两槽组件与 13 项安全场景、6 项契约、16 项网关/HTTP、4 项快照、Broker/API 回归和全-target Clippy 通过；修正恢复 Host 夹具后的完整最终结果见 #43，不包含新增 HTTP Skill 六方法/SSE 的验收 |

流式实现保留单调时钟 deadline、请求作用域数据库取消和实际后端退出断言；
[流式说明](m3-skill-streams.md)记录其已交付 profile。main 的失败与后续修复单独跟踪，
不以历史绿灯覆盖最新失败。HTTP 核心组件的完整两槽运行及 13 项安全场景已分别通过，
包含原 5 秒 Skill、同键重放、双层拒绝、实际 SSE、精确撤权、跨 owner 和冻结上下文拒绝。
此前四路 debug 试验保留失败记录；签名制品/真实 OCI 四槽混合场景的基础结果见 #43 精确 CI。
issuer-account 到期、缓存/快照与当前来源/计费回归已有证据；新增 #45–47 仍待精确最终 CI。
M3-A 完整副作用、M3-B 完整 HTTP Skill 与 M3-D/E/F 未全部验收，不能宣布 M3 完成。

本地检查命令为 `python3 tools/issues/validate.py`、
`python3 -m unittest discover -s tools/issues -p 'test_*.py' -v`、
`python3 docs/tools/validate_docs.py --strict`、`git diff --check`。
严格依赖使用隔离 venv 安装自仓库 requirements；这些结果不替代真实 OCI/浏览器测试。
环境能力须实际执行核定，资源不足时保留可复现工具和缺失证据，不伪造通过。
托管 OCI 测试保留合成 node_budget 与真实插件进程/cgroup 的明确边界，并保留 4C16G 容量不足时生产准入拒绝；
F 中实际读取两实例 CPU/内存/PID 限额及重启后复核，也须等对应门禁成功才成为证据。
它不等于合规容量 daemon 启动、SIGKILL/reexec、持续压力或独立宿主恢复验收。

## DEFERRED：AWS 与独立宿主正式验收

按本轮维护者指令，真实 AWS S3、依赖 AWS 的格式升级/备份回滚、
正式 AWS off-host 认证、独立 AWS 资源灾难恢复全部 **DEFERRED**。
它们不阻断 M3 开发；保留实现、测试入口和公开版本化制品的现有门禁。
不得伪造 `off_host=true`、Ready 或恢复成功，也不将跳过测试算作通过。
本地兼容对象存储、隔离 PostgreSQL、loopback XFS、真实 OCI 与故障注入仍可执行。

恢复执行条件：维护者重新授权真实 AWS 演练，隔离资源、凭证、独立密钥与
可销毁合规宿主准备完成；按[现有 AWS 入口](m2-aws-format-acceptance.md)执行并保存真实日志。
非 AWS 的 [RUN-03](run03-host.md) 不因 AWS 暂缓而取消；
只有具备合规独占容量、内核真实 4C16G 保留和压力条件时才可验收。
独立宿主恢复须另备真实独立目标及密钥；当前未执行时保持待验收。

## 完成判定与后续依赖

只有所有 M3 核心功能及适用真实测试完成、必需 PR 合并且 main 最终 CI 成功，
才可声明 M3 implementation complete；正式生产验收尚缺时附 production acceptance pending。
若 Skill、代理、OAuth、WebUI、UI 桥或实际端到端仍缺，则继续开发，不能使用该完成状态。
最终报告必须列出 main SHA、阶段/PR、模块、测试/CI、已实现与未实现操作、
重大未解决问题和明确暂缓验收。

M4 接续依赖为经验证的固定 Skill/部署绑定、实时来源授权、可取消调用、
声明式网关及 UI 生命周期；后续才实现完整依赖解析、HostService、持久事件和订阅。
Batch/Workflow/MPR/Extension 的未来桥方法保留契约，在依赖实际交付前明确不可用。
