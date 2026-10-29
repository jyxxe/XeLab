# ChatChoice

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ChatChoice.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

文本生成结果

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `index` | integer | 是 | minimum=`0`; maximum=`0` | 本版固定 0 |
| `message` | [TextMessage](TextMessage.md) | 是 | 见类型定义 | 兼容接口仅支持文本消息 |
| `finish_reason` | string | 是 | enum=`["stop", "length"]` | 结束原因 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "index",
    "message",
    "finish_reason"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
