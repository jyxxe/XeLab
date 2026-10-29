# EmbeddingResponse

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/EmbeddingResponse.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

批量向量结果

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `object` | string | 是 | enum=`["list"]` | 类型 |
| `model` | string | 是 | minLength=`1`; maxLength=`128` | 模型别名 |
| `data` | array&lt;[EmbeddingItem](EmbeddingItem.md)&gt; | 是 | minItems=`1`; maxItems=`128` | 逐输入对应的向量 |
| `usage` | [TokenUsage](TokenUsage.md) | 是 | 见类型定义 | 插件提供并由网关校验的计量；不作为授权判断 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "object",
    "model",
    "data",
    "usage"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
