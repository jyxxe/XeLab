# authorizeOAuth · 浏览器授权与回调

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`GET /oauth/authorize`

使用内核 __Host-xelab_auth Cookie；未登录先显示可信登录页，登录后显示授权页。拒绝非法回调地址且绝不重定向；批准后 302 回调 code/state；拒绝时回调 error=access_denied/state。授权码 60s 单次使用，绑定客户端、URI、PKCE 及固定授权范围。

| 项目 | 定义 |
|---|---|
| 权限动作 | `trusted_login_and_consent`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | 本接口定义的公开/登录凭证流程 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`account`。目标取值：collection：逐对象过滤；创建对象以已验证 subject 为 proposed owner。

默认拒绝；校验对象归属、完整祖先/源凭证链、固定上限、当前授权版本和额度；特殊字段条件见接口说明。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `response_type` | query | 是 | string; enum=`["code"]` | 本版固定 code |
| `client_id` | query | 是 | string; minLength=`1`; maxLength=`128` | 已注册客户端 |
| `redirect_uri` | query | 是 | string; format=`"uri"`; minLength=`1`; maxLength=`2048` | 精确注册回调 URI |
| `state` | query | 是 | string; minLength=`1`; maxLength=`256` | 客户端防 CSRF 随机值 |
| `code_challenge` | query | 是 | string; minLength=`43`; maxLength=`43`; pattern=`"^[A-Za-z0-9_-]{43}$"` | S256 PKCE challenge |
| `code_challenge_method` | query | 是 | string; enum=`["S256"]` | 固定 S256 |
| `scope` | query | 是 | string; minLength=`1`; maxLength=`512` | 申请的权限组/规则句柄，必须提前注册 |

请求体：无。发送非空请求体返回 `400 INVALID_ARGUMENT`。

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 302 | 无响应体 | 成功 |
| 400 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 请求或字段非法 |
| 401 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 未认证、凭证失效或来源链失效 |
| 429 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 调用配额/限流 |
| 500 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内部错误，不泄露敏感信息 |
| 503 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内核未就绪或后端不可用 |
