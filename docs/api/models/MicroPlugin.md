# MicroPlugin

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroPlugin.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

第一类核心资源；无独立OCI容器和Deployment资源预留

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `owner_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `name` | string | 是 | minLength=`1`; maxLength=`64`; pattern=`"^[a-z][a-z0-9-]{0,63}$"` | 稳定名称，创建后不变 |
| `display_name` | string | 是 | minLength=`1`; maxLength=`128` | 显示名 |
| `status` | string | 是 | enum=`["active", "archived"]` | 资源状态 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "owner_id",
    "name",
    "display_name",
    "status"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
