# Task

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Task.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

一次工作流执行；流程版本及授权来源不可原地修改

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `owner_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `workflow_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `delegation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `state` | string | 是 | enum=`["queued", "running", "waiting_dependency", "paused_authorization", "cancel_requested", "succeeded", "failed", "canceled", "unknown"]` | 状态 |
| `input` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `output` | [JsonValue](JsonValue.md) / null | 是 | 见类型定义 | 可为空 |
| `deadline_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 整体期限 |
| `error` | [ErrorDetail](ErrorDetail.md) / null | 是 | 见类型定义 | 可为空 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "owner_id",
    "workflow_id",
    "delegation_id",
    "state",
    "input",
    "output",
    "deadline_at",
    "error"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
