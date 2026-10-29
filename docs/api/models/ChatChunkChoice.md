# ChatChunkChoice

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ChatChunkChoice.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

流式单个 choice

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `index` | integer | 是 | minimum=`0`; maximum=`0` | 固定 0 |
| `delta` | [ChatDelta](ChatDelta.md) | 是 | 见类型定义 | 文本流增量；首帧 role=assistant，中间帧 content，终帧可为空 |
| `finish_reason` | string / null | 是 | 见类型定义 | 最后一帧前为空 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "index",
    "delta",
    "finish_reason"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
