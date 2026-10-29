# CreateChild

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateChild.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

创建属于路径账户的子账户；实际授权还受调用凭证限制

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `display_name` | string | 是 | minLength=`1`; maxLength=`80` | 子账户显示名称 |
| `kind` | string | 否 | default=`"user"`; enum=`["user", "service"]` | 默认 user |
| `inheritance` | [InheritanceSpec](InheritanceSpec.md) | 是 | 见类型定义 | 用户选择的继承方式；快照上限由内核展开并保存 |
| `expires_at` | string | 否 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 可选账户到期时间 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "display_name",
    "inheritance"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
