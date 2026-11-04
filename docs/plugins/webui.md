# WebUI 插件与可信登录边界

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 生命周期和路由

WebUI 为独立 OCI 部署，资源计入插件池，可安装、禁用、升级和替换。内核启动、管理 API 就绪及 CLI 恢复均不等待 WebUI。

清单声明 WebUiSpec，转发在 `/ui/{plugin_id}/` 内。该路径只定义路由，不构成浏览器隔离。不同不可信 UI 使用独立 origin 或不授同源权限的 sandbox；禁止与可信登录来源共享可读令牌存储。

`WebUiSpec.origin=null` 表示由平台按固定部署 UUID 和 ui_gateway 配置分配独立 HTTPS 来源。签名镜像在部署创建前无法预知该 UUID，因此不要求发布者预填一个不可实现的域名；非空 origin 仍必须与该固定部署的分配值完全相同。内核域、其他插件域或任意 URL 均不能作为覆盖值。Client 的 origin/redirect 注册和默认 UI 跳转随后核验该实际分配值，不能按插件名称或 latest 动态换源。

官方壳先创建 disabled 部署再注册其精确 Client 来源。初建业务配置可以保留 client_id=null；取得公开 Client ID 后，通过受限 updateDeployment 写入完整配置，再审批和启用。null 不能使运行中插件 Ready；Initialize 仍要求有效 Client ID。此引导不使用虚假的外部回调，也不要求在安装前预知内核生成的部署 UUID。

需要动态承载第三方贡献的所有OCI WebUI壳均使用这一公开运行时配置键：`config.client_id` 必须是该固定部署实际使用的active公共Client ID。核心按持久OAuth access的Client/Origin绑定与该配置精确比较，不能从多个matching Client中任选。该framing profile同样适用于非默认自定义壳；没有对应绑定时不签发私有动态父源政策，静态公开壳和CLI仍可独立工作。[贡献规范](ui-contributions.md)规定证明的固定query与当前审批绑定；证明不是用户身份或业务授权。

`/api/platform/v1/`、`/oauth/`、`/auth/ui/`、`/health/` 为内核保留，插件路由不能覆盖。平台 `/` GET 仅 302 到 DefaultUi 的入口；未配置或 UI 不可用返回 503/明确错误，不影响管理 API。

## 公共客户端

浏览器不保存 client_secret。client_id 仅为公开应用标识，不能证明后端实例身份。用户跳转内核最小可信登录/授权页，使用 code + PKCE S256；code 60s 单次，访问令牌 15min。

客户端令牌绑定 subject、client、audience 和固定授权上限。刷新令牌最长 24h，每次使用轮换，检测旧令牌重用后吊销同族。用户撤权和祖先禁用仍需逐请求生效。

有 BFF 后端时，其工作负载身份另行验证，用户委托仍不能被后端已有权限放大。所有管理动作由内核授权并审计；WebUI 无数据库、运行时 socket 或宿主特权挂载。

## 界面必需行为

展示角色与权限组来源、父级限制原因、子账户树、快照与跟随模式区别；密钥只展示首次明文。禁用自己或切换默认 UI 前展示可用 CLI 路径，不私自改变授权。

登录页、授权页属于内核安全入口，不把整个管理控制台嵌入内核。来源及 CSRF 规则见 [身份认证](../identity/authentication.md)。

## 桌面外观与插件贡献

默认预设使用类似macOS的桌面交互，支持可变主题、Dock、窗口和桌面组件，详见[WebUI详细目录](../preset-plugins/webui/README.md)。任意插件可选择Manifest.ui发布iframe窗口和/或组件，不要求自己成为默认WebUI。接入规则见[UI贡献](ui-contributions.md)与[消息桥](../communication/ui-bridge.md)。

批处理通过[核心异步batch](../skills/batches.md)提交/查询；关闭桌面、窗口或组件不取消已接受任务。
