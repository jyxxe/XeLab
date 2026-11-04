# 声明式 HTTP 与静态界面代理

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 契约注册

RouteSpec.kind=http 必须声明 http_contract_digest；镜像内 `/xelab/http/openapi.json` 提供该插件的完整 OpenAPI 3.1 接口，摘要与清单匹配，路径全部位于 `/plugins/{plugin_id}/`。每个方法都必须定义请求、响应、错误及 required_action，安装时拒绝无结构的任意透传路由。

平台只注册插件 OpenAPI 明确声明的路径/方法，前缀不是 catch-all 授权。插件第三方业务接口由它自己的契约定义；内核公开接口清单不假定能预先列出未来插件的业务对象。

操作权限沿用平台 OpenAPI 的 `x-permission` 字符串，必须与匹配的签名 RouteSpec.required_action 完全相同；不另增可由调用方覆盖的权限字段。业务动作只允许 `plugin.http.invoke` 或本插件 `plugin_id.` 命名空间内已注册且资源类型为 plugin 的动作，不能借用其他核心管理权限。审批和每次调用仍查询当前权限定义并使用现有 Authorizer。首个实现 profile 仅接受字面路径、自包含 JSON Schema 和显式成功/错误状态，路径模板、查询参数、外部 Schema 引用及 SSE 不在该 profile 内，未实现时明确拒绝，不隐式透传。

static/webui 仅提供获批 HTTP 文件及页面，GET/HEAD 转发受清单前缀和来源规则限制；业务写接口仍须 http 契约。容器内静态文件路径由插件负责，但内核拒绝路径穿越、编码斜杠和头部注入。

## 转发规则

外部身份头和原始用户 Bearer 被移除；网关建立目标插件可验证的固定委托。允许的请求头为 Content-Type、Accept、If-None-Match、Range（仅静态实现支持时）及契约列出的非敏感自定义头。hop-by-hop 头禁止转发。

响应按契约校验状态、类型、长度和最大时间。插件 Set-Cookie 仅允许自己的隔离来源和路径，不能为内核登录域或更广父域设置 Cookie。重定向不得冒充内核授权回调。

WebSocket 不属于本版通用路由协议；HTTP/SSE 已覆盖明确需求。未来增加时要定义握手授权、撤权、消息上限和心跳，不通过普通反向代理配置隐式开放。

## 在途行为

禁用路由后拒绝新请求，在途按 deadline/drain 策略处理。流式输出已开始后禁止切换提供者。插件退出时关闭无效连接并传播取消，不能让网关无限保留句柄。
