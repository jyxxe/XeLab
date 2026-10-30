# iframe宿主消息桥 v1

[通信目录](README.md) · [插件贡献](../plugins/ui-contributions.md) · [消息Schema](../contracts/ui-bridge.schema.json) · [逐方法](ui-bridge-methods.md)

> 规格版本：2.8.0；状态：待实现的规范。协议名xelab.ui/1，不是原生系统接口。

## 握手与封装

宿主为每个挂载生成UUID view_id与256bit随机channel_nonce，连同protocol和精确parent_origin写入frame URL fragment；这些只关联通道，不是授权凭证。iframe从fragment取得父源，仅向该精确origin发送ready。宿主检查后发送host.init，包含resolved主题令牌、减少动画/透明度、可见状态、viewport、locale、贡献摘要及当前获准bindings。

所有消息是普通JSON对象，禁止函数、Transferable、任意原型对象或可执行字符串。共同必填protocol、view_id、channel_nonce、seq、type；seq为每个方向独立递增的安全整数，ready/host.init各从1开始。10s未握手则显示加载失败，业务请求在init之前一律拒绝。

收发均检查event.source是否当前iframe.contentWindow/parent、event.origin是否精确注册源、版本/view_id/nonce、seq及Schema。不得postMessage到"*"，不接受null origin。来源/通道不符静默丢弃并限量记录，不向伪造源回复。重载frame必须生成新通道，旧nonce立即失效。

请求上限1MiB UTF-8 JSON；响应上限16MiB+64KiB封装开销，单个业务结果仍限16MiB；拒绝过深结构（最大64层）、非有限数字、重复JSON键或额外字段。宿主可先JSON安全化再验证，不能将postMessage对象原样拼入DOM/URL。每view最多4个在途请求、每分钟120次，超限RATE_LIMITED；正常轮询按1..30s退避。

## 请求与响应

request必填request_id（当前view内递增正整数）、method、params。method为封闭集合，精确输入/输出见[逐方法](ui-bridge-methods.md)。iframe只给binding_alias，宿主映射到本次init获准Skill UUID/digest/modes；不能自行给URL、任意skill_id、主体、凭证、契约或执行服务账户。

response回显request_id/method，ok=true仅有result；ok=false仅有error。错误包含code/message/retryable；后端错误额外包含http_status、backend_error(ErrorDetail)、core_request_id，保留后端稳定错误，不暴露令牌/堆栈/内部路径。

每个方向seq重复/倒退拒绝；新请求的request_id必须严格大于此前已接受的编号，重发保留原编号；需要重发时使用新seq及同request_id。同request_id且规范请求一致复用原等待或缓存结果，未完成时不另发未定义的处理中响应；异内容返回IDEMPOTENCY_CONFLICT，不能重复调用后端。宿主保留当前view最近256条/15min去重元数据和已接受编号高水位；低于高水位且缓存已淘汰的编号拒绝为CHANNEL_CLOSED，调用方按后端稳定幂等键核实，不能默认为新业务。

写入/提交/取消须显式idempotency_key；宿主以主体+安装+贡献+方法+调用方键的JCS摘要派生核心HTTP键，不因视图重载改变。核心幂等作用域仍含credential_id，24h内同来源同键同请求才保证去重，换内容冲突；换登录来源后应先从任务中心恢复批次，不能假定跨凭证去重。同步call的Rpc业务键与HTTP键一致；网络超时不代表副作用未发生。

## 事件与关闭

宿主event仅支持theme.changed、visibility.changed、viewport.changed、widget.refresh和host.dispose，params逐类型定义在Schema；事件不携带凭证。主题和尺寸事件合并为每帧最多一次，不能触发重复业务写入。

window.close/组件卸载/退出/升级销毁通道与UI等待回调；已提交batch继续由核心执行，UI回调丢失不取消其业务。未完成同步调用走取消传播/结果核实；不将Promise取消当作写入回滚。

允许列表、后台刷新、焦点与窗口限制由WebUI壳实施，是iframe隔离要求；内核按实际用户/客户端、完整源链和Skill原权限逐请求授权。任何框架内“已验证消息”都不能替代核心授权。WebUI本身不持有所有用户的管理后门。

本地桥错误：PROTOCOL_ERROR、CHANNEL_CLOSED、METHOD_NOT_ALLOWED、BINDING_UNAVAILABLE、PERMISSION_DENIED、PAYLOAD_TOO_LARGE、RATE_LIMITED、DEADLINE_EXCEEDED、IDEMPOTENCY_CONFLICT；核心失败用BACKEND_ERROR并带原ErrorDetail。未知method不回退到通用HTTP代理。

## 扩展插件边界

Extension Plugin不会继承宿主iframe的origin、Cookie、登录令牌、channel_nonce或Skill允许列表，也不把扩展源码注入桌面/宿主DOM。现消息桥仍只使用本次握手明确批准的固定Skill绑定；逻辑微/扩展宿主没有可任意借用的网络origin。

扩展Skill若被显式注册并获准作为业务绑定，核心仍逐次检查扩展、完整宿主链和handler权限；binding_alias、host_chain_digest与准备句柄不能代替授权。不提供从扩展页面到任意宿主管理HTTP或HostService的通用桥。
