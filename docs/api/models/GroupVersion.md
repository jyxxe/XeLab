# GroupVersion

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/GroupVersion.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

不可变权限组版本

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `group_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `version` | integer | 是 | minimum=`1`; maximum=`2147483647` | 版本号 |
| `rules` | array&lt;[PermissionRule](PermissionRule.md)&gt; | 是 | minItems=`1`; maxItems=`256` | 完整规则 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 发布时间 |
| `created_by` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "group_id",
    "version",
    "rules",
    "created_at",
    "created_by"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
