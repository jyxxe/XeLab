# TaskStep

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/TaskStep.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

任务节点状态

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `task_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `step_key` | string | 是 | minLength=`1`; maxLength=`80` | WorkflowStep.id |
| `state` | string | 是 | enum=`["pending", "ready", "running", "succeeded", "failed", "canceled", "unknown"]` | 状态 |
| `provider_deployment_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |
| `attempt_count` | integer | 是 | minimum=`0`; maximum=`5` | 已创建尝试数量 |

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
    "task_id",
    "step_key",
    "state",
    "provider_deployment_id",
    "attempt_count"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
