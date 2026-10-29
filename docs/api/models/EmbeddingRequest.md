# EmbeddingRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/EmbeddingRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

文本 embedding 子集

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `model` | string | 是 | minLength=`1`; maxLength=`128` | 模型别名 |
| `input` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`128` | 文本批次 |
| `encoding_format` | string | 否 | default=`"float"`; enum=`["float"]` | 仅支持 float |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "model",
    "input"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
