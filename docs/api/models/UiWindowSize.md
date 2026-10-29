# UiWindowSize

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiWindowSize.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

窗口逻辑像素约束，min<=default<=max；小屏由宿主裁剪到可用工作区

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `width` | integer | 是 | minimum=`320`; maximum=`4096` | 初始宽 |
| `height` | integer | 是 | minimum=`200`; maximum=`2160` | 初始高 |
| `min_width` | integer | 是 | minimum=`320`; maximum=`4096` | 最小宽 |
| `min_height` | integer | 是 | minimum=`200`; maximum=`2160` | 最小高 |
| `max_width` | integer | 是 | minimum=`320`; maximum=`4096` | 最大宽 |
| `max_height` | integer | 是 | minimum=`200`; maximum=`2160` | 最大高 |
| `resizable` | boolean | 是 | default=`true` | 是否允许用户调整 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "width",
    "height",
    "min_width",
    "min_height",
    "max_width",
    "max_height",
    "resizable"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
