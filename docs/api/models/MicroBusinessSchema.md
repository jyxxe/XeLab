# MicroBusinessSchema

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroBusinessSchema.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

受限JSON Schema2020-12；每份<=64KiB、<=256节点/深度16，无$ref/正则/自定义format或资源能力标记

## 字段

类型：object / boolean。受限JSON Schema2020-12；每份<=64KiB、<=256节点/深度16，无$ref/正则/自定义format或资源能力标记

## 对象及跨字段约束

```json
{
  "type": [
    "object",
    "boolean"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
