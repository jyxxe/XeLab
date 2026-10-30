# updateMicroDeploymentSettings · 手动设置微部署CPU、内存与联网开关

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`PUT /api/platform/v1/micro-deployments/{micro_deployment_id}/settings`

要求micro.deploy及父部署micro.runtime.use，If-Match强ETag与Idempotency-Key。只在enabled=false、phase=disabled、控制器已排空且无worker/队列/Prepare/旧句柄租约时接受完整resources与network_enabled；否则409 STATE_CONFLICT。资源每微部署聚合：缺省250mCPU/256MiB，JS heap仍为独立子限额；不得超过当前MPR工作池/节点准入或不能容纳业务子限额。network_enabled=true只允许controlled-http-v1，且不授予权限/Secret或任意网络能力。持久化实质变更时revision/generation同事务递增并失效旧句柄；重复同值不增加代次。原limits、版本、父绑定、candidate_skill和已注册Skill保持不变；启用时重新Prepare并按当前代次检查。返回200为已保存设置，不表示已启用或预留了节点资源。

| 项目 | 定义 |
|---|---|
| 权限动作 | `micro.deploy`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`micro_plugin`。目标取值：路径对象/版本/微部署/Call经核心读取并解析到真实micro_plugin UUID；集合按当前资源scope过滤，创建owner取subject。

解析真实micro_plugin UUID并检查micro.deploy及完整父级/源凭证上限；另检查固定runtime_deployment_id上的micro.runtime.use与当前审批。联网开关不是micro.http.request授权，出网时仍检查调用者、批准HttpBinding/secret、父egress和完整来源链。

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
| `resources` | [MicroResourceLimits](../../models/MicroResourceLimits.md) | 是 | 见类型定义 | 同一微部署全部活动worker合计的cgroup CPU/内存硬限制，不是JS heap或每Call复制预算；由处理器在创建时补齐整对象缺省值 |
| `network_enabled` | boolean | 是 | default=`false` | 显式联网意图；true只允许已声明controlled-http-v1，仍须调用权限/HttpBinding/父egress；false拒绝HTTP profile Invoke与ProxyHttp |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "resources": {
    "cpu_millis": 250,
    "memory_bytes": 268435456
  },
  "network_enabled": false
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 200 | `application/json` → [MicroDeployment](../../models/MicroDeployment.md) | 配置已持久化；微部署仍保持disabled，资源仅在活动调用准入时预留 |
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
