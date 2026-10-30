# createExtensionPlugin · 创建扩展插件身份

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/extension-plugins`

owner取当前认证主体；name在owner内唯一。创建身份不绑定宿主、不创建OCI、不发布贡献或注册Skill。

| 项目 | 定义 |
|---|---|
| 权限动作 | `extension.create`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`extension_plugin`。目标取值：核心读取身份/版本/扩展部署以解析真实extension_plugin UUID；集合过滤当前scope，owner来自认证主体。

检查当前祖先/凭证上限；创建或执行另需每一typed宿主的extension.host.use。micro root另需micro.invoke与micro.runtime.use；固定handler另查skill.invoke与其实际目标动作。没有宿主权限继承或任意调用旁路。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `Idempotency-Key` | header | 是 | string; minLength=`1`; maxLength=`128` | 原请求键，24h 去重 |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`64`; pattern=`"^[a-z][a-z0-9-]{0,63}$"` | 稳定名称，创建后不变 |
| `display_name` | string | 是 | minLength=`1`; maxLength=`128` | 普通文本显示名 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "name": "json-echo",
  "display_name": "example"
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 201 | `application/json` → [ExtensionPlugin](../../models/ExtensionPlugin.md) | 成功 |
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

## 并发、重试与副作用

`Idempotency-Key` 在副作用发生前持久化。相同键及相同规范请求返回同一结果或 Operation；请求内容不同返回 `409 IDEMPOTENCY_CONFLICT`。传输超时只允许用原键查询/重试，不能换键重放未知副作用。

所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。
