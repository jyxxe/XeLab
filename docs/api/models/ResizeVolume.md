# ResizeVolume

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ResizeVolume.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

仅支持扩容；仍须宿主存储准入

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `capacity_bytes` | integer | 是 | minimum=`1048576`; maximum=`9007199254740991` | 新硬配额，必须大于当前容量 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "capacity_bytes"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
