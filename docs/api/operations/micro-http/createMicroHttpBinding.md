# createMicroHttpBinding · 绑定微服务商与托管密钥

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/micro-deployments/{micro_deployment_id}/http-bindings`

If-Match属于父MicroDeployment。仅controlled-http-v1可创建，alias必须来自固定微清单且唯一；创建默认disabled，需secret.bind及当前Secret有效/父出口审批，配置不发送请求。

| 项目 | 定义 |
|---|---|
| 权限动作 | `micro.http.bind`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`micro_plugin`。目标取值：核心由真实micro_deployment/HttpBinding解析到实际micro_plugin，owner不可自报。

完整来源链、微资源scope与当前受控HTTP声明；修改需micro.http.bind，创建/换secret另需secret.bind及父MPR出口审批。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `micro_deployment_id` | path | 是 | string; format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `Idempotency-Key` | header | 是 | string; minLength=`1`; maxLength=`128` | 原请求键，24h 去重 |
| `If-Match` | header | 是 | string; minLength=`1`; maxLength=`64`; pattern=`"^\"rev-[1-9][0-9]*\"$"` | 来自最近读取响应的强 ETag，格式 "rev-{revision}" |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `endpoint_alias` | string | 是 | minLength=`1`; maxLength=`32`; pattern=`"^[a-z][a-z0-9-]{0,31}$"` | 不可变微清单中的endpoint别名 |
| `priority` | integer | 否 | minimum=`0`; maximum=`1000`; default=`10` | 越小越优先；同值按binding UUID稳定排序 |
| `enabled` | boolean | 否 | default=`false` |  |
| `secret_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 由secret.bind批准，只供核心代理注入 |
| `timeout_ms` | integer | 否 | minimum=`100`; maximum=`4000`; default=`4000` | 单次上游总时限，仍受根deadline剩余约束 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "endpoint_alias": "brave",
  "secret_id": "00000000-0000-4000-8000-000000000001"
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 201 | `application/json` → [MicroHttpBinding](../../models/MicroHttpBinding.md) | 已提交元数据；运行就绪需读取phase |
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
