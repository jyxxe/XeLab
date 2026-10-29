# 插件可选窗口与桌面组件

[插件目录](README.md) · [WebUI预设](../preset-plugins/webui/README.md) · [清单](manifest.md)

> 规格版本：2.6.0；状态：待实现的规范。

## 与桌面壳分开

Manifest.webui表示可作为默认管理入口的WebUI壳；Manifest.ui表示任意插件贡献给壳的页面。两者独立：普通业务插件可只提供iframe窗口、只提供widget、两者都提供或均不提供。禁用插件UI不应导致其健康业务Skill被判定不可用。

PluginUiSpec.protocol_version固定"1"，base_path必须为/ui/{plugin_id}/，contributions为1..16个UiContributionSpec且id唯一。窗口需window尺寸且禁止widget字段；组件需widget网格尺寸且禁止window字段。完整字段：[PluginUiSpec](../api/models/PluginUiSpec.md)、[贡献模型](../api/models/UiContributionSpec.md)。

清单只声明容器内静态入口，不提交任意外部URL。entry_path/icon_path以/起始，相对UI根拼接；拒绝..段、反斜线、双斜线、转义路径、query/fragment和scheme。核心安装与每次路由解析均规范化校验，不能只凭正则认为安全。

## 安装与发现

入口必须位于已批准static/webui GET/HEAD路由，静态页面不含私有数据；业务API仍经Skill。新窗口/组件、Skill绑定或页面代码变化属于新镜像/清单版本；不能在运行时发布未签名贡献。

部署获得平台分配的独立HTTPS origin，与可信认证端、WebUI壳及其他不互信插件不同；publisher不能指定内核域或其他插件域。源映射由[ui_gateway配置](../operations/configuration.md)确定，数据面只访问内部端点；未配置隔离origin时贡献unavailable，业务接口仍可用。

[listUiContributions](../api/operations/ui/listUiContributions.md)返回当前用户plugin.ui.open范围内已批准项，包含固定安装、manifest/contribution摘要、frame_url/origin及当前获准的bindings。getUiContribution在每次加载/重新握手核对当前版本。没有独立的“插件自报UI已获权”入口。

skill_bindings仅可引用本插件清单skills内精确name/version；alias唯一，modes明确call/batch。外部依赖由该插件经HostService正规调用，不让前端随意声明其他插件管理Skill。未注册/未启用/无调用权的绑定不出现在bindings中；spec保留原签名声明供说明，不把spec当实际授权白名单。

用户可获准打开静态界面而暂不拥有其中业务调用权，页面需显示授权/依赖状态。plugin.ui.open不隐含plugin.read、plugin.configure、skill.invoke或batch.create；Guest也只能打开明确共享的贡献。

## 不可信页面边界

窗口和widget均用iframe，禁止eval/动态import插件脚本进桌面壳，禁止插件直接读取父页面DOM、Cookie、localStorage或登录令牌。组件小并不意味着更可信，也不能请求原生宿主权限。

宿主使用sandbox="allow-scripts allow-same-origin"，前提是iframe与壳严格跨源；不授予top-navigation、popups、forms、downloads等权限。没有隔离origin时不得用同源iframe凑合。该协议v1不接受opaque/null origin，避免使用通配postMessage。

插件静态响应与网关共同设置CSP：frame-ancestors仅获准壳源，default-src 'none'，script/style/font来自本插件静态源（禁止unsafe-eval/内联脚本），img/media只允许本源和blob，connect-src 'none'，object-src 'none'，base-uri 'none'，form-action 'none'。宿主iframe设置referrerpolicy="no-referrer"，外部HTTP也不允许跳转到其他源。

插件页面只通过[消息桥](../communication/ui-bridge.md)请求已声明Skill。桥允许列表是桌面壳必须实现的页面隔离规则，不能替代核心授权；内核每次仍检查完整业务权限。WebUI本身也是经用户授权的公共客户端，不能因为担任宿主就获得Owner凭证。

示例：[知识库窗口与索引组件声明](../contracts/examples/plugin-ui.json)。它是可校验设计片段，当前预设镜像和对应HTML尚未实现；不会把示例当已部署页面。
