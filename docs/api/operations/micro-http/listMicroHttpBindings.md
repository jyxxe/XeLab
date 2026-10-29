# listMicroHttpBindings · 列出微部署HTTP服务商配置

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`GET /api/platform/v1/micro-deployments/{micro_deployment_id}/http-bindings`

仅列此微部署获准绑定，最多8项；管理视图可见secret_id但无原值。运行中的脚本只获取不含secret_id的冻结投影。

| 项目 | 定义 |
|---|---|
| 权限动作 | `micro.http.read`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`micro_plugin`。目标取值：核心由真实micro_deployment/HttpBinding解析到实际micro_plugin，owner不可自报。

完整来源链、微资源scope与当前受控HTTP声明；修改需micro.http.bind，创建/换secret另需secret.bind及父MPR出口审批。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `micro_deployment_id` | path | 是 | string; format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |

请求体：无。发送非空请求体返回 `400 INVALID_ARGUMENT`。

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 200 | `application/json` → [MicroHttpBindingPage](../../models/MicroHttpBindingPage.md) | 已提交元数据；运行就绪需读取phase |
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
