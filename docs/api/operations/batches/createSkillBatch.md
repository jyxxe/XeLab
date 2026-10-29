# createSkillBatch · 提交异步Skill批次

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/skills/{skill_id}/batches`

batch统一走异步入口；验证全部输入、固定Skill/安装和执行委托后原子持久化，返回202、Location及Retry-After，不等待推理/写入完成。最多1000项/4MiB；每项仍遵守原有界接口与实例并发。必须同时拥有skill.invoke和目标原权限。

| 项目 | 定义 |
|---|---|
| 权限动作 | `batch.create`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`skill_batch`。目标取值：拟创建skill_batch的owner为提交者；path.skill_id及实际run_as主体另校验，不接受正文owner/目标覆盖。

重新检查当前账户/来源链/客户端上限；批次另检查固定执行委托和Skill/目标原授权；UI发现不授予业务权。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `skill_id` | path | 是 | string; format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `Idempotency-Key` | header | 是 | string; minLength=`1`; maxLength=`128` | 原请求键，24h 去重 |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `items` | array&lt;[SkillBatchInputItem](../../models/SkillBatchInputItem.md)&gt; | 是 | minItems=`1`; maxItems=`1000` | 提交前全部按目标输入Schema校验；同批id重复拒绝 |
| `item_timeout_ms` | integer | 否 | minimum=`100`; maximum=`300000`; default=`120000` | 显式值不超过Skill上限；省略取min(120000,Skill.maximum_timeout_ms) |
| `timeout_ms` | integer | 否 | minimum=`1000`; maximum=`604800000`; default=`86400000` | 整批期限，不能延长源授权有效期 |
| `max_parallel` | integer | 否 | minimum=`1`; maximum=`16`; default=`1` | 整批在途项目上限，实际还受用户/实例/节点额度限制 |
| `failure_policy` | string | 否 | default=`"continue"`; enum=`["continue", "stop"]` | continue继续独立项目；stop停止领取新项目，不回滚已提交项 |
| `run_as` | string | 否 | default=`"user"`; enum=`["user", "service"]` | 默认 user；service 需要独立批准 |
| `service_account_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | run_as=service 时必填 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "items": [
    {
      "id": "example",
      "input": {}
    }
  ]
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 202 | `application/json` → [SkillBatch](../../models/SkillBatch.md) | 已持久化提交，稍后查询进度和结果 |
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

异步业务批次：`202` 返回 `SkillBatch`；`Location` 指向查询入口，`Retry-After` 给出轮询间隔。持久化后即结束提交连接，进度和结果使用 [batch 协议](../../../skills/batches.md) 查询。

## 并发、重试与副作用

`Idempotency-Key` 在副作用发生前持久化。相同键及相同规范请求返回同一结果或 Operation；请求内容不同返回 `409 IDEMPOTENCY_CONFLICT`。传输超时只允许用原键查询/重试，不能换键重放未知副作用。

所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。
