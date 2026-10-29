# ChatRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ChatRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

平台定义的 chat/completions 文本子集；不支持字段拒绝，不静默丢弃

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `model` | string | 是 | minLength=`1`; maxLength=`128` | 管理员配置的模型别名 |
| `messages` | array&lt;[TextMessage](TextMessage.md)&gt; | 是 | minItems=`1`; maxItems=`256` | 会话消息 |
| `stream` | boolean | 否 | default=`false` | 默认 false |
| `temperature` | number | 否 | minimum=`0`; maximum=`2`; default=`1` | 采样温度，默认 1 |
| `max_tokens` | integer | 否 | minimum=`1`; maximum=`32768`; default=`1024` | 生成 token 上限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "model",
    "messages"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
