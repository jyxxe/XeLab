# MicroSource

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroSource.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

原始UTF8 JavaScript源码，原始字节<=256KiB，返回text/javascript

## 字段

类型：string。原始UTF8 JavaScript源码，原始字节<=256KiB，返回text/javascript

## 对象及跨字段约束

```json
{
  "type": "string",
  "minLength": 1,
  "maxLength": 262144
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
