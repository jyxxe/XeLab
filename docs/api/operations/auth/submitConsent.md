# submitConsent · 批准或拒绝浏览器委托

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/auth/consents`

仅当前 Cookie 会话可决定绑定该会话的固定 pending 授权请求。JSON 使用 X-CSRF-Token 与 Idempotency-Key 请求头；可信页面也可原生提交 URL 编码表单，将这两个同名字段作为传输元数据，验证后移除再按 ConsentDecision 校验。重复或冲突来源拒绝。一次性 code/Location 不缓存，重复批准不能重取明文。

| 项目 | 定义 |
|---|---|
| 权限动作 | `self`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | 仅内核可信 Cookie 会话，敏感操作额外校验 CSRF；不接受 API Key |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`account`。目标取值：认证或授权请求确定的当前账户。

默认拒绝；校验对象归属、完整祖先/源凭证链、固定上限、当前授权版本和额度；特殊字段条件见接口说明。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `Idempotency-Key` | header | 否 | string; minLength=`1`; maxLength=`128` | JSON 必须提供；原生表单必须在请求头或同名表单字段二选一提供。24h 去重；一次性授权码与 Location 不保存为可重放明文。 |
| `X-CSRF-Token` | header | 否 | string; minLength=`43`; maxLength=`128` | Cookie 敏感操作必须提供会话绑定的 CSRF 证明；JSON 使用此头，原生表单可用同名传输字段，重复或冲突来源拒绝。 |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `authorization_request_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `approved` | boolean | 是 | 见类型定义 | 是否批准预览中的固定权限范围 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "authorization_request_id": "00000000-0000-4000-8000-000000000001",
  "approved": false
}
```

也接受 Content-Type：`application/x-www-form-urlencoded`；传输元数据与互斥来源规则见接口说明。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `authorization_request_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `approved` | string | 是 | enum=`["true", "false"]` | 明确允许或拒绝固定范围，不接受其他 truthy 表示 |
| `X-CSRF-Token` | string | 否 | minLength=`43`; maxLength=`128` | 与请求头二选一的会话 CSRF 证明 |
| `Idempotency-Key` | string | 否 | minLength=`1`; maxLength=`128` | 与请求头二选一的 24h 去重键 |

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 302 | 无响应体 | 成功 |
| 400 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 请求或字段非法 |
| 401 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 未认证、凭证失效或来源链失效 |
| 403 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 动作或范围未授权 |
| 404 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 资源不存在或不可见 |
| 409 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 状态冲突、配额冲突或一次性响应无法重放 |
| 413 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 请求超过大小限制 |
| 415 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 不支持的 Content-Type |
| 429 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 调用配额/限流 |
| 500 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内部错误，不泄露敏感信息 |
| 503 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内核未就绪或后端不可用 |
| 504 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 调用整体期限已到 |

一次性响应：签发成功后不存储可重放明文；同键重试返回 `409 ONE_TIME_RESPONSE_UNAVAILABLE` 并可带资源标识。客户端应查询元数据，无法恢复明文时撤销并重新签发。

## 并发、重试与副作用

`Idempotency-Key` 在副作用发生前持久化。相同键及相同规范请求返回同一结果或 Operation；请求内容不同返回 `409 IDEMPOTENCY_CONFLICT`。传输超时只允许用原键查询/重试，不能换键重放未知副作用。

所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。
