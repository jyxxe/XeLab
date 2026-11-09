# M3 当前验收记录（2026-11-09，北京时间）

[实施索引](README.md) · [开发历史](m3-progress.md) · [目标 #38](https://github.com/jyxxe/XeLab/issues/38)

M3-A 至 M3-E 的代码及适用真实验收已有证据。以下三个精确功能提交的完整与专项 CI 均已最终成功；主线集成、收尾文档及最终 main CI 仍须逐项核对，最新集成 SHA 和完成判定由[目标 #38](https://github.com/jyxxe/XeLab/issues/38)维护。通过功能分支 CI 不等于已经完成最终主线门禁。

| 阶段 | 已实现与验证 | 对应交付 |
|---|---|---|
| Phase 0 / M1–M2 | Issue 机制、适用身份/配额/运行时/存储/恢复回归；修复 Broker 撤权活性、严格捕获容量及测试观察竞态 | #31、#39–44；下列精确完整 CI |
| M3-A | 七项 Skill 操作；固定签名安装/UUID/版本/摘要、双重授权；普通四类副作用的单次与声明流、原实例 QueryResult、未知结果围栏 | #39、#40、#41、#46 |
| M3-B | 声明式 HTTP/OpenAPI、受控静态 GET/HEAD、六种 HTTP Skill 方法、严格 SSE、取消/撤权和共享并发 | #43、#47 |
| M3-C | Client 管理、原生可信登录/同意、code/PKCE、刷新轮换/重用撤族、默认 UI 及 CLI 恢复 | #44；下列真实 OAuth/桌面 CI |
| M3-D | 独立签名 OCI `org.xelab.webui`、桌面/窗口/主题/管理及真实状态，公开授权 API，无数据库后门 | #45 |
| M3-E | 固定 window/widget、独立来源 iframe、nonce/source/origin/seq、固定 alias 的 `skill.call` 和同部署 `window.open` | #45 |
| M3-F 运行证据 | Owner/Child 真实 OAuth、Skill、非默认壳、窗口/组件、攻击拒绝、撤权、停用后 CLI、持久状态与核心组件重启 | 三个精确提交各完整/专项成功，下面分别列出 |
| M3-F 集成门禁 | 功能 PR 顺序集成、收尾文档 PR 和最终 main 自身 CI | 以目标 #38 的实际最终 SHA/CI 为准，不能从旧绿灯推定 |

| 精确功能提交 | Tree | 完整 / 专项 CI |
|---|---|---|
| [#45](https://github.com/jyxxe/XeLab/pull/45) `63c3b5e43ed6a1e095261f8831ba54c41bdc7848` | `3a917476d6a180275d4d60bd52ae706660f529df` | [41932091819](https://github.com/jyxxe/XeLab/actions/runs/41932091819)，均成功 |
| [#46](https://github.com/jyxxe/XeLab/pull/46) `aedc6d9749f61c3cb0b41fbd7f3c12d36bf0b8f4` | `2ba28ea9a162f132bc755057f475767f2efdb819` | [41932118847](https://github.com/jyxxe/XeLab/actions/runs/41932118847)，均成功 |
| [#47](https://github.com/jyxxe/XeLab/pull/47) `e4af36ad8f4479f6bec71aeba8741cb38ecd5efd` | `cb84245bf1692644c6dd92a670f78a5eae7786af` | [41932137350](https://github.com/jyxxe/XeLab/actions/runs/41932137350)，均成功 |

每份专项原日志已核实 57 项浏览器与 13 项 Core 检查全通过：5 个唯一完整 Call JSON、6 次 POST/6 个完成事件、无失败/未完成正文或隐私异常。包括实际清空默认 UI、直接重载非默认壳并挂载贡献、恢复默认供子账户根路径登录；不是静态 HTML 或编译结果。完整作业还执行真实 OCI、XFS/进程崩溃恢复、默认 workspace、严格文档、协议、fmt/Clippy 和 tracked 文件检查。完整日志与精确 CI 是对应结论的依据，不能把默认 ignored 数量当作实际组件执行证据。

### 已修复的安全与稳定性问题

公开 CSP 不再枚举其他账户的部署来源。非默认动态壳通过真实 OAuth Client/Origin 和当前 `plugin.ui.open` 后取得 framing-only 精确父政策；query 在 Core 校验后剥离，不传给插件，也不包含用户/会话/凭据 ID 或授予业务权限。UI 游标使用独立目的 AES-GCM，SQL 在 LIMIT 前按当前 Policy 与实际 owner 预选；固定 ID/owned_by 的合法交集保留，组合授权在分配增长前执行原 4MiB 上限。实际 PostgreSQL/独立 mTLS 正负回归已执行，完整作业复核这些回归。

XFS 崩溃测试仅把启动轮询改为无锁、有界、逐层 O_NOFOLLOW 观察原子标记；随后仍执行完整 XFS proof、inode/检查点与原业务/配额断言，再真实 SIGABRT 和新进程恢复。阶段并非分别输出独立日志，SIGABRT 日志、新进程恢复 sentinel 与必经源码断言共同构成证据。生产存储锁、deadline、unknown 账本和物理围栏均未放宽。

HTTP/2 读取保持原字节上限、严格 UTF-8/JSON/type、deadline 和取消。真实 Chromium/TLS/H2 组件与真实 Core/OCI 是两类证据，不能互相替代。历史登录前静态 504 的具体底层原因未被独立证明；新增固定阶段诊断，当前精确门禁成功不改写历史失败，也不证明整个运行期从未发生诊断事件。

### 可用操作、边界及复现

可用 Skill 操作为 `registerSkill/getSkill/listSkills/setSkillEnabled/archiveSkill/invokeSkill/streamSkill`。Client 的创建/读取/列表/修改/禁用、默认 UI 读取/设置、贡献发现及本人偏好沿用公开管理 API。运行代码分别在 `crates/kernel/src/{skills,skill_streams,broker,broker_query,broker_http,plugin_http_gateway,oauth,ui,webui}.rs` 及 `oauth/`；独立执行代码在 `examples/webui/`、`examples/ui-reference/`、`examples/http/` 和普通 OCI 参考制品。

部署、精确 DNS/TLS/Client 引导、真实 CLI 恢复和独立执行入口见 [WebUI 实现与复现](m3-webui.md)及[部署步骤](../../examples/webui/DEPLOYMENT.md)。Schema 为 16；迁移仅追加，已发布 SQL 校验和保持。公开版本化数据制品准入继续关闭。

HostService Skill 代理/完整依赖解析、持久事件/订阅属于 M4；BatchEngine、Workflow、MPR、Extension 和知识库业务仍未交付，桥的未来方法明确不可用。XeLibrary 不是本项目依赖。同源多 Client 的默认跳转仍按通用部署契约选择：官方壳配置的 Client 被禁用、另一个同源 Client 仍 active 时可能先跳壳再显示 OAuth 拒绝；没有权限复活，CLI/API 独立，此为后续 P2 可用性改进，尚无双 Client 实测结论。

真实 AWS 按维护者要求 **DEFERRED**。正式 RUN-03、daemon SIGKILL/reexec、独立宿主恢复及完整生产格式验收仍 pending；恢复条件和执行入口保留于[原验收边界](m3-progress.md)。真实插件 cgroup 读数不等于正式 4C16G 容量认证，合成 node_budget 与同进程核心组件重建也不等于生产容量、OS 重启或 off-host 认证。
