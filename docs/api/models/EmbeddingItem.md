# EmbeddingItem

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/EmbeddingItem.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

向量输出

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `object` | string | 是 | enum=`["embedding"]` | 类型 |
| `index` | integer | 是 | minimum=`0`; maximum=`127` | 与输入顺序一致 |
| `embedding` | array&lt;number&gt; | 是 | minItems=`1`; maxItems=`65536` | 向量维度由模型固定 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "object",
    "index",
    "embedding"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
