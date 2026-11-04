# 插件可选窗口与桌面组件

[插件目录](README.md) · [WebUI预设](../preset-plugins/webui/README.md) · [清单](manifest.md)

> 规格版本：2.8.0；状态：待实现的规范。

## 与桌面壳分开

Manifest.webui表示可作为默认管理入口的WebUI壳；Manifest.ui表示任意插件贡献给壳的页面。两者独立：普通业务插件可只提供iframe窗口、只提供widget、两者都提供或均不提供。禁用插件UI不应导致其健康业务Skill被判定不可用。

PluginUiSpec.protocol_version固定"1"，base_path必须为/ui/{plugin_id}/，contributions为1..16个UiContributionSpec且id唯一。窗口需window尺寸且禁止widget字段；组件需widget网格尺寸且禁止window字段。完整字段：[PluginUiSpec](../api/models/PluginUiSpec.md)、[贡献模型](../api/models/UiContributionSpec.md)。

清单只声明容器内静态入口，不提交任意外部URL。entry_path/icon_path以/起始，相对UI根拼接；拒绝..段、反斜线、双斜线、转义路径、query/fragment和scheme。核心安装与每次路由解析均规范化校验，不能只凭正则认为安全。

## 安装与发现

入口必须位于已批准static/webui GET/HEAD路由，静态页面不含私有数据；业务API仍经Skill。新窗口/组件、Skill绑定或页面代码变化属于新镜像/清单版本；不能在运行时发布未签名贡献。

部署获得平台分配的独立HTTPS origin，与可信认证端、WebUI壳及其他不互信插件不同；publisher不能指定内核域或其他插件域。源映射由[ui_gateway配置](../operations/configuration.md)确定，数据面只访问内部端点；未配置隔离origin时贡献unavailable，业务接口仍可用。

[listUiContributions](../api/operations/ui/listUiContributions.md)返回当前用户plugin.ui.open范围内已批准项，包含固定安装、manifest/contribution摘要、frame_url/origin及当前获准的bindings。getUiContribution在每次加载/重新握手核对当前版本。没有独立的“插件自报UI已获权”入口。contribution_digest 是该精确 UiContributionSpec 的 RFC8785/JCS SHA-256，部署、安装和 manifest_digest 另行固定比较；同一安装的不同部署不能互换。

分页游标是不透明、带完整性保护的密文，不把不可见插件、部署或贡献标识放进可解码明文。游标绑定当前主体、凭证来源链、Origin、对端、过滤和页大小及授权版本；跨来源、篡改或授权变化须从首页重新读取。先用现Policy的当前选择器按实际部署owner预选，再有界扫描并逐项复核；不能让其他账户的私有行通过空页/游标暴露数量桶，也不能丢掉owned_by与固定插件ID交集下的合法共享贡献。合法候选的实时变化产生空页时仍保留正常续页进展。旧明文签名游标不得作为保密游标继续接受。

skill_bindings仅可引用本插件清单skills内精确name/version；alias唯一，modes明确call/batch。外部依赖由该插件经HostService正规调用，不让前端随意声明其他插件管理Skill。未注册/未启用/无调用权的绑定不出现在bindings中；spec保留原签名声明供说明，不把spec当实际授权白名单。

用户可获准打开静态界面而暂不拥有其中业务调用权，页面需显示授权/依赖状态。按当前用户权限过滤而省略绑定不会单独使静态贡献 unavailable；全局目标候选或版本缺失、路由未批准、源隔离或实例失效仍须拒绝挂载。plugin.ui.open不隐含plugin.read、plugin.configure、skill.invoke或batch.create；Guest也只能打开明确共享的贡献。

## 不可信页面边界

窗口和widget均用iframe，禁止eval/动态import插件脚本进桌面壳，禁止插件直接读取父页面DOM、Cookie、localStorage或登录令牌。组件小并不意味着更可信，也不能请求原生宿主权限。

宿主使用sandbox="allow-scripts allow-same-origin"，前提是iframe与壳严格跨源；不授予top-navigation、popups、forms、downloads等权限。没有隔离origin时不得用同源iframe凑合。该协议v1不接受opaque/null origin，避免使用通配postMessage。

插件静态响应与网关共同设置CSP：frame-ancestors仅获准壳源，default-src 'none'，script/style/font来自本插件静态源（禁止unsafe-eval/内联脚本），img/media只允许本源和blob，connect-src 'none'，object-src 'none'，base-uri 'none'，form-action 'none'。宿主iframe设置referrerpolicy="no-referrer"，外部HTTP也不允许跳转到其他源。

公开静态响应不得枚举其他账户的部署来源。壳的frame-src固定为平台专用ui_gateway HTTPS命名空间，可信内核登录源必须在该命名空间之外；这不替代网关Host解析、部署审批或当前用户的贡献发现授权。普通页面的frame-ancestors只公布显式配置的公开壳源和当前可用的公开默认WebUI，不从全局部署目录生成。

非默认的动态WebUI仍可挂载获准贡献。核心在真实OAuth access的持久Client/Origin绑定与当前plugin.ui.open检查后，可为frame_url附加唯一保留参数 `xelab_frame=v1.<selector>.<mac>`。selector是仅含public client_id、origin_index和签名contribution_id的RFC8785/JCS规范JSON之UTF-8字节的无填充base64url；client_id为1至128个字符且不含Unicode Cc控制字符，origin_index为0至7的整数。mac为32字节、无填充base64url的目的隔离HMAC。值最多1024字符，selector最多768字符；未知、重复、非规范或歧义编码均拒绝。selector不包含用户、会话、家族或凭据标识；两端部署、安装、签名摘要、入口、审批和当前Client/revision只参与核心内部绑定。网关重核这些绑定及精确父源，去除整个参数后才转发固定插件路径，并只把该精确动态父源加入CSP。

该证明只选择公开页面的嵌入政策，不是核心凭据，不能用于贡献发现、Skill或管理授权。访问令牌刷新不改变未变化的政策；Client禁用/修改、两端审批或制品变化使旧证明失效。登出、用户撤权及业务请求仍沿用现有消息桥与核心Authorizer。CLI不能用自报Origin取得私有动态壳证明。publisher的entry_path/icon_path仍一律禁止query；只有核心生成的frame_url允许上述固定例外，不能添加任意URL或业务参数。

插件页面只通过[消息桥](../communication/ui-bridge.md)请求已声明Skill。桥允许列表是桌面壳必须实现的页面隔离规则，不能替代核心授权；内核每次仍检查完整业务权限。WebUI本身也是经用户授权的公共客户端，不能因为担任宿主就获得Owner凭证。

示例：[知识库窗口与索引组件声明](../contracts/examples/plugin-ui.json)。它是可校验设计片段，当前预设镜像和对应HTML尚未实现；不会把示例当已部署页面。

## 扩展不继承UI来源

Extension Plugin扩展业务功能不自动取得宿主UI origin、Cookie、令牌或iframe贡献权限，也不能把JS注入桌面壳或宿主页面。逻辑宿主为微插件或另一扩展时仍无独立网络origin；没有明确支持的静态来源和UI贡献协议时，扩展仅以受授权Skill/宿主管理界面操作。

如将来扩展需要独立UI贡献，必须另行声明不可变页面制品、来源隔离、Skill绑定和加载授权；不能以“属于同一宿主”复用其源或绕过当前消息桥。
