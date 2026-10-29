# CreateGroup

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateGroup.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

建立组并发布 v1；所有规则须在实际调用主体可授权范围内

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 组名称 |
| `visibility` | string | 否 | default=`"private"`; enum=`["private", "public"]` | 默认 private |
| `rules` | array&lt;[PermissionRule](PermissionRule.md)&gt; | 是 | minItems=`1`; maxItems=`256` | 组规则 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "rules"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
