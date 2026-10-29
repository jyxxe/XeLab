# RuleConditions

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/RuleConditions.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

规则附加约束；省略表示没有该项约束

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `not_before` | string | 否 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最早生效时间 |
| `expires_at` | string | 否 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 到期时间 |
| `source_cidrs` | array&lt;string&gt; | 否 | minItems=`1`; maxItems=`32` | 允许的请求来源网段 |
| `audiences` | array&lt;string&gt; | 否 | minItems=`1`; maxItems=`32` | 允许接收凭证的服务 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": []
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
