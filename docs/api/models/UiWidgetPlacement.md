# UiWidgetPlacement

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiWidgetPlacement.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

同用户实例；column/row为0起始网格坐标；重叠需宿主整理

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `instance_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `contribution` | [UiContributionKey](UiContributionKey.md) | 是 | 见类型定义 | 用户布局只引用具体部署与贡献，不跨新安装静默恢复 |
| `column` | integer | 是 | minimum=`0`; maximum=`255` | 列 |
| `row` | integer | 是 | minimum=`0`; maximum=`255` | 行 |
| `columns` | integer | 是 | minimum=`1`; maximum=`8` | 宽格数，受贡献min/max限制 |
| `rows` | integer | 是 | minimum=`1`; maximum=`8` | 高格数，受贡献min/max限制 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "instance_id",
    "contribution",
    "column",
    "row",
    "columns",
    "rows"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
