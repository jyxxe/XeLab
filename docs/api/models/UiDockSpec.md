# UiDockSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiDockSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

应用Dock偏好；pinned中同贡献不得重复

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `position` | string | 是 | default=`"bottom"`; enum=`["bottom", "left", "right"]` | Dock位置 |
| `auto_hide` | boolean | 是 | default=`false` | 自动隐藏 |
| `pinned` | array&lt;[UiContributionKey](UiContributionKey.md)&gt; | 是 | minItems=`0`; maxItems=`64` | 固定的插件窗口；仅window种类可固定 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "position",
    "auto_hide",
    "pinned"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
