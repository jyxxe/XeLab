# CredentialConditions

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CredentialConditions.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

密钥附加条件，创建者源凭证条件也持续适用

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `expires_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 必需的绝对到期时间，默认策略最大 365 天 |
| `source_cidrs` | array&lt;string&gt; | 否 | minItems=`1`; maxItems=`32` | 来源网段 |
| `audiences` | array&lt;string&gt; | 否 | minItems=`1`; maxItems=`32` | 目标服务 |
| `max_calls` | integer | 否 | minimum=`1`; maximum=`9007199254740991` | 终生调用上限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "expires_at"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
