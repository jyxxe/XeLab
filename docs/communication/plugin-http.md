# 声明式 HTTP 与静态界面代理

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 契约注册

RouteSpec.kind=http 必须声明 http_contract_digest；镜像内 `/xelab/http/openapi.json` 提供该插件的完整 OpenAPI 3.1 接口，摘要与清单匹配，路径全部位于 `/plugins/{plugin_id}/`。每个方法都必须定义请求、响应、错误及 required_action，安装时拒绝无结构的任意透传路由。

平台只注册插件 OpenAPI 明确声明的路径/方法，前缀不是 catch-all 授权。插件第三方业务接口由它自己的契约定义；内核公开接口清单不假定能预先列出未来插件的业务对象。

操作权限沿用平台 OpenAPI 的 `x-permission` 字符串，必须与匹配的签名 RouteSpec.required_action 完全相同；不另增可由调用方覆盖的权限字段。业务动作只允许 `plugin.http.invoke` 或本插件 `plugin_id.` 命名空间内已注册且资源类型为 plugin 的动作，不能借用其他核心管理权限。审批和每次调用仍查询当前权限定义并使用现有 Authorizer。实现 profile 接受字面路径、自包含 JSON Schema、显式成功/错误状态和下述 SSE；路径模板、查询参数、外部 Schema 引用、业务 HEAD 和静态 Range 明确拒绝，不隐式透传。

static/webui 仅提供获批 HTTP 文件及页面，GET/HEAD 转发受清单前缀和来源规则限制；业务写接口仍须 http 契约。容器内静态文件路径由插件负责，但内核拒绝路径穿越、编码斜杠和头部注入。

## 转发规则

外部身份头和原始用户 Bearer 被移除；网关建立目标插件可验证的固定委托。允许的请求头为 Content-Type、Accept、If-None-Match、Range（仅静态实现支持时）及契约列出的非敏感自定义头。hop-by-hop 头禁止转发。

内核在 mTLS 通道重建 `X-Xelab-Delegation`（既有不透明 RpcDelegation）、`X-Xelab-Subject`（已认证主体 UUID）、`X-Xelab-Call-ID`（已持久化 Call UUID）、`X-Xelab-Deadline-Unix-Ms`（根剩余期限对应的十进制 Unix 毫秒）及 `X-Xelab-Idempotency-Key`。后者为持久化业务键 UTF-8 的无填充 base64url，不是外部 HTTP 去重头的盲目透传；Skill 显式业务键与网关 HTTP 去重键保持既有分离语义。外部同名头全部丢弃，插件不能通过声明自定义头取得这些保留字段。插件只信任通过平台 CA 验证、精确内核 SPIFFE 的来源，不离线解释委托角色。

直接 HTTP 的内部 Call 契约为 `{name: plugin_id + ".http", version: 插件精确版本, schema_digest: http_contract_digest}`，仅用于统一账本，不能作为能力提供者或裸 RPC 注册。HTTP Skill 继续使用 Skill.name/version/definition_digest 的虚拟 ContractRef。每次调用冻结安装、部署、实例代次和 operation；相同插件路径有多个获批部署且没有精确分配来源时拒绝，不选择最新版。重放仍验证当前固定目标、原始授权来源、审批和健康状态，不能读取已停用目标的旧结果。

响应按契约校验状态、类型、长度和最大时间。插件 Set-Cookie 仅允许自己的隔离来源和路径，不能为内核登录域或更广父域设置 Cookie。重定向不得冒充内核授权回调。

WebSocket 不属于本版通用路由协议；HTTP/SSE 已覆盖明确需求。未来增加时要定义握手授权、撤权、消息上限和心跳，不通过普通反向代理配置隐式开放。

## 在途行为

禁用路由后拒绝新请求，在途按 deadline/drain 策略处理。流式输出已开始后禁止切换提供者。插件退出时关闭无效连接并传播取消，不能让网关无限保留句柄。

声明式 `200 text/event-stream` 的媒体 Schema 描述每个 `data` 的自包含 StreamEvent，`event/id/data` 必须一致，accepted 首帧、sequence 从 1 连续递增、恰好一个终帧。内核逐帧验证签名 Schema、固定 Call/提供者及当前权限，确认 EOF 和持久化成功后才交付 completed；EOF 缺终帧、终帧后继续数据或结构错误均失败。禁止 Last-Event-ID 恢复，沿用[通用背压上限](streaming.md)，心跳不构成业务授权。HTTP Skill 同样可固定到已声明的 SSE 操作，外部响应使用 SkillStreamEvent，后端仍只能产生原 HTTP StreamEvent，不能自报 Skill UUID/摘要。

HTTP Skill 的 identity 映射只使用固定操作的方法、字面路径及 JSON 正文，不将业务输入转换成 URL、查询或请求头。GET 等无正文操作要求 `input=null`，204 结果要求可验证的 null 输出；JSON 操作的所有已声明成功状态须与固定 Skill 输出 Schema 一致，不仅限于 POST/200。流式候选的 OpenAPI 自包含 StreamEvent Schema 还须通过标准 JSON Schema 的按 kind 条件分支明确约束 `item.payload` 和 `completed.payload.output`，分别与签名 Skill 的 item/output Schema 精确一致；通用 JsonValue 信封不充当业务 Schema。注册时比较，执行时同时验证完整签名事件、Skill item/output、固定调用和提供者。完成后的虚拟 ContractRef 由核心构造，插件只证明原 `plugin_id.http`/版本/HTTP 契约摘要。流中断/取消不证明副作用已回滚，Call 保持 unknown 及原提供者围栏；相同流键只允许核查原 Call，不重新发送或拼接。

所有真实业务转发复用 Broker 的实例共享并发、祖先配额、来源凭证计费和 Call 围栏。静态 GET/HEAD 使用同一物理实例池的轻量请求预留，不伪造公共主体或用户日调用；已确认传输结束或已证明进程停止后才释放。用户断线或请求结果不明关闭上游并保持未知预留至物理围栏，不自动重试。响应 Body 的独立有界容量持续到 Body 被销毁，不随物理 Call 提前回收。
