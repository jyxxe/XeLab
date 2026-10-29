# RolePage

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/RolePage.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

按公共约定及资源专属稳定顺序分页；仅包含当前主体可见对象

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `items` | array&lt;[Role](Role.md)&gt; | 是 | minItems=`0`; maxItems=`200` | 本页对象 |
| `next_cursor` | string / null | 是 | 见类型定义 | 绑定主体、过滤条件和创建水位的游标，15min 到期 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "items",
    "next_cursor"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
