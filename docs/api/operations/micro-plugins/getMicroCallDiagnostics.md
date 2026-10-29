# getMicroCallDiagnostics · 读取本人或获准微调用诊断

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`GET /api/platform/v1/micro-calls/{call_id}/diagnostics`

同时要求实际micro资源上的micro.diagnostics.read与具体call上的capability.call.read，避免脚本所有者偷看其他调用者数据。诊断最长保留7天，到期410；普通运行时日志不得包含私有stdin/stdout/stderr。

| 项目 | 定义 |
|---|---|
| 权限动作 | `micro.diagnostics.read`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`micro_plugin`。目标取值：路径对象/版本/微部署/Call经核心读取并解析到真实micro_plugin UUID；集合按当前资源scope过滤，创建owner取subject。

检查完整父级/源凭证/客户端上限与实际资源；创建/执行微部署另需micro.runtime.use(实际父deployment)，发布另需blob.read；不按普通OCI资源动作猜测授权。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `call_id` | path | 是 | string; format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |

请求体：无。发送非空请求体返回 `400 INVALID_ARGUMENT`。

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 200 | `application/json` → [MicroCallDiagnostics](../../models/MicroCallDiagnostics.md) | 成功 |
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
| 410 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 微调用诊断保留期已过 |
