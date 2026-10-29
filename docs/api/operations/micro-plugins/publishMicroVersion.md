# publishMicroVersion · 发布固定微脚本制品

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/micro-plugins/{micro_plugin_id}/versions`

验证Blob读取权、源码字节摘要/UTF8/容量和有效JSON Schema后复制到核心不可变存储，计算package_digest。版本不覆盖；同manifest.version仅完全相同制品/运行时安装可幂等确认。元数据验证不执行脚本，语法/编译失败在Prepare阶段报告。

| 项目 | 定义 |
|---|---|
| 权限动作 | `micro.publish`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`micro_plugin`。目标取值：路径对象/版本/微部署/Call经核心读取并解析到真实micro_plugin UUID；集合按当前资源scope过滤，创建owner取subject。

检查完整父级/源凭证/客户端上限与实际资源；创建/执行微部署另需micro.runtime.use(实际父deployment)，发布另需blob.read；不按普通OCI资源动作猜测授权。

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `micro_plugin_id` | path | 是 | string; format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `Idempotency-Key` | header | 是 | string; minLength=`1`; maxLength=`128` | 原请求键，24h 去重 |
| `If-Match` | header | 是 | string; minLength=`1`; maxLength=`64`; pattern=`"^\"rev-[1-9][0-9]*\"$"` | 来自最近读取响应的强 ETag，格式 "rev-{revision}" |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `manifest` | [MicroManifest](../../models/MicroManifest.md) | 是 | 见类型定义 | 核心独立保存的不可变微制品清单；不是OCI Manifest |
| `source_blob_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 需当前blob.read；源码<=256KiB UTF8且无NUL/BOM |
| `source_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 原始源码字节SHA-256，不规范化代码 |
| `runtime_installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 精确已安装且声明Manifest.micro_runtime的OCI制品 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "manifest": {
    "format_version": "1",
    "version": "1.0.0",
    "description": "example",
    "engine": "javascript",
    "protocol": "json-stdio-v1",
    "entrypoint": "main.js",
    "input_schema": {},
    "output_schema": {},
    "limits": {
      "timeout_ms": 2000,
      "heap_bytes": 16777216,
      "stack_bytes": 1048576,
      "maximum_input_bytes": 1048576,
      "maximum_output_bytes": 1048576,
      "maximum_stderr_bytes": 65536,
      "max_active_requests": 1
    },
    "side_effect": "none"
  },
  "source_blob_id": "00000000-0000-4000-8000-000000000001",
  "source_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "runtime_installation_id": "00000000-0000-4000-8000-000000000001"
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 201 | `application/json` → [MicroPluginVersion](../../models/MicroPluginVersion.md) | 成功 |
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
