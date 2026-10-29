# ChatChunk

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ChatChunk.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

兼容流式增量；结束后发送 data: [DONE]

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | minLength=`1`; maxLength=`80` | chatcmpl_ + UUID |
| `object` | string | 是 | enum=`["chat.completion.chunk"]` | 类型 |
| `created` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | Unix 秒 |
| `model` | string | 是 | minLength=`1`; maxLength=`128` | 模型别名 |
| `choices` | array&lt;[ChatChunkChoice](ChatChunkChoice.md)&gt; | 是 | minItems=`1`; maxItems=`1` | 单个 choice |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "object",
    "created",
    "model",
    "choices"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
