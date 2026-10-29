# UiWidgetSize

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiWidgetSize.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

组件占用桌面网格，1格=96逻辑像素，间距12；不控制全局桌面

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `columns` | integer | 是 | minimum=`1`; maximum=`8` | 默认列数 |
| `rows` | integer | 是 | minimum=`1`; maximum=`8` | 默认行数 |
| `min_columns` | integer | 是 | minimum=`1`; maximum=`8` | 最小列数 |
| `min_rows` | integer | 是 | minimum=`1`; maximum=`8` | 最小行数 |
| `max_columns` | integer | 是 | minimum=`1`; maximum=`8` | 最大列数 |
| `max_rows` | integer | 是 | minimum=`1`; maximum=`8` | 最大行数 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "columns",
    "rows",
    "min_columns",
    "min_rows",
    "max_columns",
    "max_rows"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
