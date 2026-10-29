# MicroHttpBindingPage

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroHttpBindingPage.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

最多8项，无隐式分页遗漏

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `items` | array&lt;[MicroHttpBinding](MicroHttpBinding.md)&gt; | 是 | minItems=`0`; maxItems=`8` | 当前获准绑定 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "items"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
