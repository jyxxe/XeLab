# ChatResponse

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ChatResponse.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

文本兼容接口非流式结果

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | minLength=`1`; maxLength=`80` | chatcmpl_ + UUID |
| `object` | string | 是 | enum=`["chat.completion"]` | 对象类型 |
| `created` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | Unix 秒 |
| `model` | string | 是 | minLength=`1`; maxLength=`128` | 模型别名 |
| `choices` | array&lt;[ChatChoice](ChatChoice.md)&gt; | 是 | minItems=`1`; maxItems=`1` | 本版仅单个 choice |
| `usage` | [TokenUsage](TokenUsage.md) | 是 | 见类型定义 | 插件提供并由网关校验的计量；不作为授权判断 |

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
    "choices",
    "usage"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
