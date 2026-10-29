# Attempt

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Attempt.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

带租约和 fencing 的单次执行

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `step_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `number` | integer | 是 | minimum=`1`; maximum=`5` | 尝试序号 |
| `execution_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 传给插件的稳定执行标识 |
| `idempotency_key` | string | 是 | minLength=`1`; maxLength=`128` | 同一步骤所有尝试保持稳定 |
| `fence` | integer | 是 | minimum=`1`; maximum=`9007199254740991` | 每次接管递增；旧执行不能提交 |
| `lease_expires_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 租约到期时间 |
| `state` | string | 是 | enum=`["leased", "running", "succeeded", "failed", "expired", "unknown"]` | 状态 |
| `checkpoint_blob_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |

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
    "step_id",
    "number",
    "execution_id",
    "idempotency_key",
    "fence",
    "lease_expires_at",
    "state",
    "checkpoint_blob_id"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
