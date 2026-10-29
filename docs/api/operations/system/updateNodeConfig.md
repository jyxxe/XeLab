# updateNodeConfig · 更新本机资源预留配置

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`PUT /api/platform/v1/node/config`

以 getNode 的 ETag 为前置条件；预留不能低于 4 vCPU/16 GiB；扩大预留导致现有插件超预算时返回 409，不自动驱逐。

| 项目 | 定义 |
|---|---|
| 权限动作 | `platform.configure`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`platform`。目标取值：本机平台单例；setDefaultUi 另校验目标部署可用。

默认拒绝；校验对象归属、完整祖先/源凭证链、固定上限、当前授权版本和额度；特殊字段条件见接口说明。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `If-Match` | header | 是 | string; minLength=`1`; maxLength=`64`; pattern=`"^\"rev-[1-9][0-9]*\"$"` | 来自最近读取响应的强 ETag，格式 "rev-{revision}" |
| `Idempotency-Key` | header | 是 | string; minLength=`1`; maxLength=`128` | 同一主体/凭证/方法/规范路径内 24h 去重；一次性密钥只在第一次返回 |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kernel_cpu_millis` | integer | 是 | minimum=`4000`; maximum=`1048576`; default=`4000` | 内核预留，至少 4 vCPU |
| `kernel_memory_bytes` | integer | 是 | minimum=`17179869184`; maximum=`9007199254740991`; default=`17179869184` | 内核预留，至少 16 GiB |
| `system_cpu_millis` | integer | 是 | minimum=`1`; maximum=`1048576` | 宿主/独立基础设施预留，必须显式配置 |
| `system_memory_bytes` | integer | 是 | minimum=`1048576`; maximum=`9007199254740991` | 宿主/基础设施预留，必须显式配置 |
| `shutdown_grace_ms` | integer | 是 | minimum=`1000`; maximum=`300000`; default=`30000` | 内核停止最大排空时间 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "kernel_cpu_millis": 4000,
  "kernel_memory_bytes": 17179869184,
  "system_cpu_millis": 1,
  "system_memory_bytes": 1048576,
  "shutdown_grace_ms": 30000
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 202 | `application/json` → [Operation](../../models/Operation.md) | 操作已持久化，轮询 Operation |
| 400 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 请求或字段非法 |
| 401 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 未认证、凭证失效或来源链失效 |
| 403 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 动作或范围未授权 |
| 404 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 资源不存在或不可见 |
| 409 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 状态冲突、配额冲突或一次性响应无法重放 |
| 412 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | If-Match 与当前版本不符 |
| 413 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 请求超过大小限制 |
| 415 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 不支持的 Content-Type |
| 428 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 缺少必需前置条件 |
| 429 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 调用配额/限流 |
| 500 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内部错误，不泄露敏感信息 |
| 503 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内核未就绪或后端不可用 |
| 504 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 调用整体期限已到 |

异步成功时 `Operation.result` 必须匹配 [NodeStatus](../../models/NodeStatus.md)；`resource_id` 指向主要结果资源。操作失败时 result=null，error 非空。

## 并发、重试与副作用

`Idempotency-Key` 在副作用发生前持久化。相同键及相同规范请求返回同一结果或 Operation；请求内容不同返回 `409 IDEMPOTENCY_CONFLICT`。传输超时只允许用原键查询/重试，不能换键重放未知副作用。

目标版本必须与 `If-Match` 一致。检查版本、当前授权、配额及写入在同一事务中完成；不一致返回 412，未提供返回 428。ETag 作用域为本目标资源；特殊子资源使用说明中指定的父资源版本。

所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。
