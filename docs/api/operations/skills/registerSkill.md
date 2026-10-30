# registerSkill · 校验并注册不可变 Skill

[接口目录](README.md) · [公共约定](../../../communication/http.md) · [错误目录](../../../communication/errors.md)

> 状态：规范性设计；并非已实现接口。结构来源为 OpenAPI，行为约束与主题文档共同构成契约。

`POST /api/platform/v1/skills`

定义必须受对应插件签名清单覆盖，名称归属该 plugin_id；输入输出与目标契约严格一致。注册不授予用户执行权，不启动插件。 installation_id必须明确；重复注册要求定义摘要和安装摘要都一致，其他制品使用新Skill版本。 micro目标仅接受核心MicroDeployment.candidate_skill原样注册，安装必须等于微版本固定runtime安装；需skill.register与micro.deploy及父运行时使用权。

| 项目 | 定义 |
|---|---|
| 权限动作 | `skill.register`；同时执行对象范围、完整祖先链及源凭证检查 |
| 认证 | Bearer 会话、用户委托或 API Key；执行前读取当前授权版本 |
| 超时 | 管理同步调用 10s；业务由 timeout_ms / 模型约束决定；异步操作不占用长 HTTP 连接 |
| 请求编码 | UTF-8；未声明字段拒绝；未声明参数拒绝；数字不得为 NaN / Infinity |

授权对象类型：`skill`。目标取值：path.skill_id 或拟注册定义；另校验目标插件/契约及全部原授权。

skill.invoke 与目标动作同时授权，保存固定定义摘要，不能经旧低层接口绕过 skill_required

## 路径、查询及请求头

| 名称 | 位置 | 必填 | 类型与约束 | 含义 |
|---|---|---|---|---|
| `Idempotency-Key` | header | 是 | string; minLength=`1`; maxLength=`128` | 原请求键，24h 去重 |

## 请求体

Content-Type：`application/json`；请求体必需。

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `definition` | [SkillDefinition](../../models/SkillDefinition.md) | 是 | 见类型定义 | 不可变的核心转发声明，不是可执行脚本或 Codex SKILL.md |
| `enabled` | boolean | 否 | default=`false` | 安装不等于启用 |
| `installation_id` | string | 是 | format=`"uuid"` | 实际物理rootOCI来源安装；micro为runtime安装，extension必须与固定宿主闭包root_installation_id相同，候选/包来源另核验 |

结构示例（标识和业务数据需替换为当前部署实际获准对象）：

```json
{
  "definition": {
    "name": "example",
    "version": "1.0.0",
    "description": "example",
    "input_schema": {},
    "output_schema": {},
    "stream_item_schema": null,
    "target": {
      "kind": "capability",
      "plugin_id": "org.xelab.echo",
      "contract": {
        "name": "example",
        "version": "1.0.0",
        "schema_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    },
    "mapping": "identity",
    "maximum_timeout_ms": 120000,
    "input_blobs": false,
    "tags": []
  },
  "installation_id": "00000000-0000-4000-8000-000000000001"
}
```

## 响应

| HTTP | 内容 | 语义 |
|---|---|---|
| 201 | `application/json` → [Skill](../../models/Skill.md) | 成功 |
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
