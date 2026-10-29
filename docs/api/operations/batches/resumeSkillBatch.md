# resumeSkillBatch · 显式重新授权暂停批次

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/skill-batches/{batch_id}/resume`

仅paused_authorization允许。必须由原owner重新批准当前来源，冻结上限与新授权求交，保留原subject/服务账户/Skill/输入/幂等键/deadline；不扩权不换所有者。只恢复未开始项，unknown不重放，已过deadline拒绝。

| 项目 | 定义 |
|---|---|
| 权限动作 | `batch.resume`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`skill_batch`。目标取值：path.batch_id经核心读取的真实skill_batch/owner；item_id属于该批次；集合逐对象过滤。

重新检查当前账户/来源链/客户端上限；批次另检查固定执行委托和Skill/目标原授权；UI发现不授予业务权。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `batch_id` | path | 是 | string; format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `Idempotency-Key` | header | 是 | string; minLength=`1`; maxLength=`128` | 原请求键，24h 去重 |
| `If-Match` | header | 是 | string; minLength=`1`; maxLength=`64`; pattern=`"^\"rev-[1-9][0-9]*\"$"` | 来自最近读取响应的强 ETag，格式 "rev-{revision}" |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 200 | `application/json` → [SkillBatch](../../models/SkillBatch.md) | 成功 |
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
| 412 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | If-Match 与当前版本不符 |
| 428 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 缺少必需前置条件 |

## 并发、重试与副作用

`Idempotency-Key` 在副作用发生前持久化。相同键及相同规范请求返回同一结果或 Operation；请求内容不同返回 `409 IDEMPOTENCY_CONFLICT`。传输超时只允许用原键查询/重试，不能换键重放未知副作用。

目标版本必须与 `If-Match` 一致。检查版本、当前授权、配额及写入在同一事务中完成；不一致返回 412，未提供返回 428。ETag 作用域为本目标资源；特殊子资源使用说明中指定的父资源版本。

所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。
