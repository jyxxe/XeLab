# Volume

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Volume.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

卷元数据

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `owner_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `spec` | [VolumeSpec](VolumeSpec.md) | 是 | 见类型定义 | 平台管理卷；禁止指定任意宿主路径 |
| `used_bytes` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | 已使用容量 |
| `state` | string | 是 | enum=`["available", "attached", "full", "deleting", "deleted"]` | 状态 |
| `attached_instance_ids` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`32` | 实际挂载实例 |

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
    "spec",
    "used_bytes",
    "state",
    "attached_instance_ids"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
