# M3 浏览器认证与 UI 控制接口


> 2026-11-09 验收补记：M3 功能提交 #45/#46/#47 的精确完整与专项 CI 已全部通过。下文保留相应切片开发时的命令、历史状态和证据边界；当前验收表与精确 head/tree/CI 见 [当前验收记录](m3-acceptance-20261109.md)，最终集成与 main CI 见 [目标 #38](https://github.com/jyxxe/XeLab/issues/38)。历史“待验收”表述不得替代该最新记录，组件通过也不替代真实 OCI 或正式生产验收。
[实施目录](README.md) · [目标与总进度](m3-progress.md) · [身份契约](../identity/authentication.md)

本页记录开发分支实现和验证边界。主线集成以对应 PR 和精确提交的 CI 为准；
这些核心接口不代表官方 OCI WebUI、第三方 iframe 或完整 M3 端到端已经验收。

## 实际控制路径

Client 注册、查询、分页、修改、禁用使用现有 Authorizer 和数据库事务。
授权只选择注册上限与当前账户权限的交集，并冻结原始来源和 Client revision；
扩大注册上限不会扩大已签发权限。禁用或修改 Client 撤销关联凭证族。
API Key 和子账户的 OAuth 来源仍进入既有来源链，不成为独立授权系统。

核心 Host 分类先于认证入口。登录和同意页在可信核心来源提供，
不加载插件脚本；登录 Cookie 是 Secure/HttpOnly/host-only。
无会话的授权请求使用短期、不透明、单次的登录 continuation，
不接受任意 next URL。原生同意表单使用请求绑定 CSRF 和幂等键，
只跳转精确注册回调；授权码和刷新凭证不进入幂等明文缓存。
PKCE 只支持 S256，code 60 秒单次；刷新凭证轮换并在重用时提交整族撤销。
访问令牌和刷新期限受原始会话及祖先来源约束。

跨来源 API 请求只接受直接、绑定 Client 来源的 OAuth access Bearer，
不接受核心登录 Cookie 或普通 Session/API Key 作为跨来源替代。
每个实际授权路径保留 Origin 条件，互不依赖的合法来源不会被另一条路径毒化。
Broker、流、Blob 及异步审批保存和复核原请求来源。

## 固定页面与恢复

WebUiSpec.origin=null 请求平台按部署 UUID 分配 HTTPS 来源；非空值必须精确相同。
静态页面来自签名路由，可信 OAuth 页面不能在插件 Host 提供。
只有该固定 WebUI 的 active Client 精确回调可以携带 code/state 或拒绝参数，
查询串只留在浏览器地址栏，转发插件前移除。
WebUI CSP 只允许连接实际核心来源；普通贡献页面保持 connect-src 'none'。
iframe 父来源来自显式配置或经过当前 Client、审批、发布者和健康实例核验的固定壳源，
不能把任意 Client origin 变成可信父页面。

默认界面是固定 deployment_id 和强 ETag。匿名根路径不创建单例、
不接受 query 或请求体；未设置、停用、无有效 Client 或不可用时返回明确 503，
核心 API 和 CLI 保持独立。

官方 WebUI 的配置引导先创建 disabled 部署，取得实际分配来源，
再注册 Client，通过 updateDeployment 写入公开 Client ID 后审批启用。
updateDeployment 的 M3 profile 只修改完全停用、无进程和保留工作的部署；
保留安装及卷身份，只支持一个副本。变更需 plugin.configure、强 ETag，
并使用现有幂等、审计、配置 Schema、资源最低限额和存储围栏。
安装切换、卷替换、运行中重配置须走其专用生命周期，不能借配置更新绕过。

## 贡献、偏好与历史数据

listUiContributions/getUiContribution 直接解析固定安装的签名贡献，
按 plugin.ui.open 过滤并返回当前双重授权的 Skill 绑定。
贡献摘要是 UiContributionSpec 的 RFC8785/JCS SHA-256。
只缺少当前用户的业务权限时可以返回 ready 和空 bindings，静态页面仍须隔离。
BatchEngine 尚未交付，不返回可执行的 batch 绑定或虚假成功。
getMyUiPreferences/setMyUiPreferences 仅操作本人偏好，持久化使用既有表、ETag 和幂等。
失效的旧布局可以作为不可执行占位保留，不能借布局重新取得访问权。

SkillInvokeRequest 的可选 provider_deployment_id 复用普通调用同名字段语义，
只收窄固定安装内的路由。桥宿主填入贡献部署，iframe 不能选择目标。
Skill/HTTP 已完成结果的读取和重放核验原始已计费来源与输入 Blob 权限；
HTTP 还核验当前审批。新的无关授权不能复活旧结果。
读取不会再次执行插件或占用业务调用额度。

追加迁移 16 注册在真实 HTTP 迁移 15 之后，不修改已发布的 1–14。
迁移不插入默认 UI 或其他种子数据，保持空目标 COPY 导入门禁。
快照导出和导入校验 OAuth/UI 结构，恢复撤销旧凭证族、Client、待处理同意和 continuation；
发布恢复就绪前再次检查历史浏览器权限没有复活。此为元数据组件边界，
不能替代文件系统、独立宿主或 off-host 认证。

## 可复现验证

设置隔离 PostgreSQL 和本地兼容对象存储测试变量后执行：

```sh
cargo test --lib oauth --locked
cargo test --lib ui::tests --locked
cargo test --lib storage::metadata_snapshot::tests --locked
cargo test --lib result_authority --locked
cargo test --lib deployments::tests --locked
cargo test --test skills --locked
cargo test --lib config::tests --locked
cargo test --lib actual_main_router_chromium_trusted_login_native_consent_pkce_cors_and_refresh_reuse --locked -- --ignored --nocapture
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 docs/tools/validate_docs.py --strict
```

浏览器测试需要 Python Playwright 和 Chromium，使用私有证书指纹及隔离测试域。
真实生产 App.router/TLS/PostgreSQL 上的 42 项检查已经执行通过，
包括密码、Cookie 隔离、原生同意、PKCE 错误、code/refresh 重放、撤族及真实账户 CORS 读取。
该测试回调页面是独立夹具，不是已安装的 OCI WebUI，不能作为完整 M3-F 证据。
迁移重放、空恢复目标、历史 OAuth COPY、UI 发现/偏好、默认界面及固定部署专项
分别保存实际结果；有失败或未执行的项目不得合并计为通过。

本地曾因 Docker PostgreSQL 的默认 64MiB /dev/shm 耗尽而中断一批测试。
原日志保留，测试服务在保留数据后改为 512MiB 并重新执行受影响项目；
没有降低内核预留、数据库验证或权限要求。真实 AWS、正式独立宿主恢复和 RUN-03
保持总进度中的 DEFERRED/待验收状态。
