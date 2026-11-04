# M3 实现目标与验收进度

[实施索引](README.md) · [目标 Issue #38](https://github.com/jyxxe/XeLab/issues/38) · [里程碑](roadmap.md)

记录日期：2026-11-04。初始基线为 main
`dbb35cbc7c2c110c88de7ba16dc2d4569fad3ad2`，内部 Schema 14。
这是开发与证据清单；未勾选项尚未完成，规范存在不代表运行代码已经交付。
后续阶段须更新实际提交、PR、测试命令及结果，不能用本页代替运行证据。

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
- [ ] Phase 0：完成本环境可运行的 M1/M2 基线回归，修复实际阻断问题。
- [ ] M3-A：registerSkill、getSkill、listSkills、setSkillEnabled、archiveSkill。
- [ ] M3-A：invokeSkill、streamSkill，经 SkillRouter → Broker → 真实固定 OCI 实例。
- [ ] M3-A：签名/安装/Schema/版本冲突、skill.invoke 与目标动作同时授权、撤权取消及共享并发。
- [ ] M3-A：独立插件注册→启用→授权→调用→禁用拒绝→恢复，覆盖重复请求、资源不足与实例重启。
- [ ] M3-B：精确签名 HTTP 路由及 OpenAPI 摘要、Schema、required_action 验证。
- [ ] M3-B：受控 GET/HEAD 静态代理、HTTP/SSE 流控、超时取消与生命周期收敛。
- [ ] M3-B：真实 HTTP 插件及伪造头、路径穿越、Cookie、重定向、未声明协议负向测试。
- [ ] M3-C：Client 注册/查询/修改/禁用、精确 origin/redirect/scope 和默认 UI。
- [ ] M3-C：可信登录/同意页、code + PKCE S256、短期单次 code、刷新轮换与重用检测。
- [ ] M3-C：会话/账户/客户端绑定、Origin/CSRF/Cookie 检查及正常/拒绝流程测试。
- [ ] M3-D：独立签名 OCI `org.xelab.webui`，可复现配置、限额、部署和访问地址。
- [ ] M3-D：桌面、Dock、启动器、多窗口、组件、主题、用户偏好、小屏及键盘可访问性。
- [ ] M3-D：插件/Skill/账户/授权/资源/Client/default UI 真实管理操作和完整状态反馈。
- [ ] M3-E：发现已批准 window/widget，固定部署、版本、贡献摘要与 binding_alias。
- [ ] M3-E：隔离 iframe、view_id/nonce/parent_origin、source/origin/version/seq 校验。
- [ ] M3-E：真正的 skill.call 与获准 window.open；撤权/登出/升级销毁旧通道。
- [ ] M3-E：独立 UI 插件与跨账户、错误来源、通道重放和权限隔离测试。
- [ ] M3-F：可信 Owner→真实插件→Skill→OAuth→WebUI→iframe→桥调用完整实际端到端。
- [ ] M3-F：撤权拒绝、停用 WebUI 后 CLI 可用、重启后持久状态/配额/边界一致。
- [ ] M3-F：自动化浏览器测试、Rust/数据库/契约/fmt/Clippy/workspace 和故障回归证据。
- [ ] M3-F：必需 PR 均合并，main 最终 CI 成功，功能/接口/迁移/文档一致。

## 已执行证据与待验收项

| 证据 | 结果及边界 |
|---|---|
| PR #31 exact head `70a8f128426c50091aaa117a961d652449129733` | [Issue checks](https://github.com/jyxxe/XeLab/actions/runs/41750527904) 与 [Kernel M1](https://github.com/jyxxe/XeLab/actions/runs/41750527932) 成功 |
| #31 与 #37 的本地合并树 | 配置校验 5 表单/22 标签；10 单元测试；strict docs 812 页；diff 检查均通过 |
| main `dbb35cbc…` 标签同步 | [Sync issue labels](https://github.com/jyxxe/XeLab/actions/runs/41796871248) 成功；已读取并核对 22 个受管标签 |
| main `dbb35cbc…` Issue 配置 | [Issue checks](https://github.com/jyxxe/XeLab/actions/runs/41796871110) 成功 |
| main `dbb35cbc…` 内核回归 | [Kernel M1](https://github.com/jyxxe/XeLab/actions/runs/41796871078) 记录时运行中，尚不能计为通过 |
| M3 运行证据 | 初始基线尚无；各阶段必须追加精确提交与实际运行记录 |

本地检查命令为 `python3 tools/issues/validate.py`、
`python3 -m unittest discover -s tools/issues -p 'test_*.py' -v`、
`python3 docs/tools/validate_docs.py --strict`、`git diff --check`。
严格依赖使用隔离 venv 安装自仓库 requirements；这些结果不替代真实 OCI/浏览器测试。
环境能力须实际执行核定，资源不足时保留可复现工具和缺失证据，不伪造通过。

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
