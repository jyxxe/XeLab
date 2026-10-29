# Backup

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Backup.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

可恢复备份清单

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `volume_ids` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`16` | 来源卷 |
| `state` | string | 是 | enum=`["creating", "ready", "failed", "expired"]` | 状态 |
| `manifest_digest` | string / null | 是 | 见类型定义 | 制品或内容 SHA-256 摘要 |
| `created_by` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `off_host` | boolean | 是 | 见类型定义 | 必须为 true 才通过生产验收 |

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
    "deployment_id",
    "volume_ids",
    "state",
    "manifest_digest",
    "created_by",
    "off_host"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
