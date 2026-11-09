# 认证、会话与凭证

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 主体与认证方式

User/ServiceAccount 是授权主体；Session/API Key 是凭证；WorkloadIdentity 是实际插件实例身份；Client 只标识 UI 应用。所有派生来源保存在内核，禁止信任 X-User-ID/X-Plugin-ID。

本机可信引导创建首个独立 Owner。待激活子用户使用 24h 单次激活 token 设置密码；服务账户不接受密码登录，使用批准的 API Key 执行后台请求。激活与签发摘要落库后明文不持久化。

密码 12–128 字符，不截断，使用成熟 Argon2id 库并为每个密码生成随机 salt；参数下限采用 OWASP 推荐的内存至少 19 MiB、迭代 2、并行度 1。登录错误统一 INVALID_CREDENTIAL，避免泄漏账户存在性。[密码存储指南](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html)

## 会话

可信 login 返回不透明会话令牌，最长 1h；浏览器同时设置内核来源的 `__Host-xelab_auth`，Secure、HttpOnly、SameSite=Lax、Path=/，不设置 Domain。第三方 UI 不调用密码登录，使用受控 OAuth code/PKCE 流。

会话令牌和 API Key 由 CSPRNG 生成至少 256 bit 随机熵；保存 HMAC-SHA256 校验摘要与 key_id，HMAC 密钥存放宿主密钥文件，不能写入数据库或日志。摘要校验采用恒定时间比较。

登出只撤销该会话。账户禁用使该账户及其后代凭证链全部停止授权；恢复账户不复活已 revoked 凭证。

## WebUI 授权

authorize 只接受精确注册 redirect_uri，PKCE 仅 S256。可信登录后由内核批准注册范围的子集并签发授权码；客户端不能自行提交账户 ID 代替会话主体。code 绑定 client/URI/challenge、60s、单次使用。

Client注册和修改时，redirect URI必须属于精确allowed_origin，且不能自带内核保留的code、state、error、error_description或xelab_frame查询字段；解码后的字段名不区分大小写检查。其他固定应用查询参数仍按原精确回调语义保留。xelab_frame仅用于核心生成的贡献嵌入政策，不是OAuth回调参数。

token 的 code 与 refresh 分支字段定义见 [OAuthTokenRequest](../api/models/OAuthTokenRequest.md)。刷新凭证每次轮换并记录族 ID，重用即吊销同族。敏感 Cookie 操作校验 CSRF token 和 Origin；CLI 无 Origin 时必须显式提供 Bearer 或密码凭证，不依据 localhost 自动放行。

未登录的 authorize 请求先校验 Client、精确回调、PKCE 与已注册 scope，再保存短期服务端续接状态；浏览器只持有内核来源的 Secure/HttpOnly、`__Host-` 不透明 Cookie。可信登录完成后，固定登录入口将该请求绑定到新会话，再跳转固定同意页。禁止任意 `next`/`return` URL，也不把密码、会话令牌或 PKCE verifier 放入续接 URL。authorize 的中间 302 只能指向内核登录/同意页，最终 302 才指向已核验回调。

scope 是排序去重的空格分隔预注册句柄：`client:<public_client_id>` 请求该 Client 的 `maximum_rules` 上限，`rule:sha256:<RFC8785规则摘要>` 请求其中一条规则。请求绑定 Client revision；实际同意范围取注册上限与当前会话有效权限的交集，并冻结原始来源。Client 注册范围不是用户权限，Guest/子账户无需持有整个 Client 上限；后续新增授权不能扩张已签发 token。

可信同意页原生提交 URL 编码表单，以便浏览器跟随一次性 302 回调。其 `authorization_request_id`、`approved` 语义与 JSON 一致，`approved` 仅接受 `true`/`false`。JSON 使用 `X-CSRF-Token`、`Idempotency-Key` 头；原生表单可使用同名传输字段，各字段与对应头必须二选一。核心先核验精确 Origin、当前 Cookie 会话、CSRF 与幂等元数据，再移除元数据并校验原有 ConsentDecision，不新增该 JSON 的字段。重复或冲突来源拒绝；一次性授权码及 Location 不持久化为可重放明文，同键重试不能重新取回 code。接口与表单 Schema 见 [submitConsent](../api/operations/auth/submitConsent.md)。

可信登录页使用 `Referrer-Policy: no-referrer`；原生同意页的响应头与 HTML meta 使用 `same-origin`，使浏览器同源 POST 保留准确 Origin，同时跨源回调不携带 Referer。不得将 `Origin: null` 当成可信内核来源，也不通过取消 Origin/CSRF 检查适配浏览器行为。

同意页的 CSP `form-action` 只包含 `'self'` 和该请求已核验的精确回调 origin，以允许浏览器跟随原生表单的跨源 302。实际 Location 仍只能由核心按注册的完整 URI 和固定请求生成；不允许通配来源或调用方提交任意目标。登录页的 `form-action` 仍仅为 `'self'`。

登录按账户与来源分别限制为每分钟 5 次失败，超限 429/Retry-After；成功不抹去审计。MFA/外部 IdP 联邦属于后续认证适配扩展，当前接口未宣称实现。
