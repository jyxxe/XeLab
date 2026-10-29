# ErrorDetail

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ErrorDetail.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

稳定错误对象

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `code` | string | 是 | minLength=`1`; maxLength=`80` | 错误码；取值见错误目录 |
| `message` | string | 是 | minLength=`1`; maxLength=`2048` | 安全且不含敏感信息的说明 |
| `retryable` | boolean | 是 | 见类型定义 | 是否允许按重试规则再次提交 |
| `violations` | array&lt;[FieldViolation](FieldViolation.md)&gt; | 是 | minItems=`0`; maxItems=`256` | 字段校验错误 |
| `retry_after_ms` | integer / null | 是 | 见类型定义 | 建议退避时间，毫秒 |
| `resource_id` | string / null | 否 | 见类型定义 | 可选非敏感资源引用；一次性签发重放供核实使用 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "code",
    "message",
    "retryable",
    "violations",
    "retry_after_ms"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
