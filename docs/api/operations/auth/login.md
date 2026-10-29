# login · 建立可信登录会话

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/auth/login`

建立可信登录会话

| 项目 | 定义 |
|---|---|
| 权限动作 | `password`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | 本接口定义的公开/登录凭证流程 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`account`。目标取值：认证或授权请求确定的当前账户。

默认拒绝；校验对象归属、完整祖先/源凭证链、固定上限、当前授权版本和额度；特殊字段条件见接口说明。

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `account_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `password` | string | 是 | minLength=`1`; maxLength=`128`; writeOnly=`true` | 账户密码 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "account_id": "00000000-0000-4000-8000-000000000001",
  "password": "example"
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 200 | `application/json` → [SessionIssued](../../models/SessionIssued.md) | 成功 |
| 400 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 请求或字段非法 |
| 401 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 未认证、凭证失效或来源链失效 |
| 429 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 调用配额/限流 |
| 500 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内部错误，不泄露敏感信息 |
| 503 | `application/json` → [ErrorEnvelope](../../models/ErrorEnvelope.md) | 内核未就绪或后端不可用 |

敏感认证响应禁止缓存或记录原始凭证。授权码仅使用一次；交换失败后按认证流程重新建立授权，不能依靠普通幂等重放取回已签发明文。

## 并发、重试与副作用

所有执行入口重新鉴权；预检成功不构成后续操作凭证。审计记录主体、实际凭证、源链、动作、资源、策略版本及结果。
