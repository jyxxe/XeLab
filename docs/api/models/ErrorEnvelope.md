# ErrorEnvelope

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ErrorEnvelope.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

平台 HTTP 错误响应

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `error` | [ErrorDetail](ErrorDetail.md) | 是 | 见类型定义 | 稳定错误对象 |
| `request_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 与 X-Request-ID 一致 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "error",
    "request_id"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
