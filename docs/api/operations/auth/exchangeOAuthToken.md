# exchangeOAuthToken · 交换授权码或轮换刷新凭证

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /oauth/token`

交换授权码或轮换刷新凭证

| 项目 | 定义 |
|---|---|
| 权限动作 | `authorization_code_or_refresh_token`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | 本接口定义的公开/登录凭证流程 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`account`。目标取值：collection：逐对象过滤；创建对象以已验证 subject 为 proposed owner。

默认拒绝；校验对象归属、完整祖先/源凭证链、固定上限、当前授权版本和额度；特殊字段条件见接口说明。

## 请求体

Content-Type：`application/x-www-form-urlencoded`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `grant_type` | string | 是 | enum=`["authorization_code", "refresh_token"]` | 授权模式 |
| `client_id` | string | 是 | minLength=`1`; maxLength=`128` | 注册客户端 |
| `code` | string | 否 | minLength=`1`; maxLength=`512` | 一次性授权码 |
| `redirect_uri` | string | 否 | format=`"uri"`; minLength=`1`; maxLength=`2048` | 必须与注册和授权请求一致 |
| `code_verifier` | string | 否 | minLength=`43`; maxLength=`128`; pattern=`"^[A-Za-z0-9._~-]{43,128}$"` | PKCE verifier，43–128 字符 |
| `refresh_token` | string | 否 | minLength=`1`; maxLength=`512` | 刷新凭证；轮换并检测重用 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "grant_type": "authorization_code",
  "client_id": "example",
  "code": "example",
  "redirect_uri": "https://ui.example.org/callback",
  "code_verifier": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 200 | `application/json` → [OAuthToken](../../models/OAuthToken.md) | 成功 |
| 400 | `application/json` → [OAuthError](../../models/OAuthError.md) | 请求或字段非法 |
| 401 | `application/json` → [OAuthError](../../models/OAuthError.md) | 未认证、凭证失效或来源链失效 |
| 429 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 调用配额/限流 |
| 500 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内部错误，不泄露敏感信息 |
| 503 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内核未就绪或后端不可用 |

敏感认证响应禁止缓存或记录原始凭证。授权码仅使用一次；交换失败后按认证流程重新建立授权，不能依靠普通幂等重放取回已签发明文。

## 并发、重试与副作用

所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。
