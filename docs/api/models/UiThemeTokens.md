# UiThemeTokens

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiThemeTokens.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

有限主题令牌；组件使用角色令牌，禁止传入任意CSS/HTML/脚本

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `accent` | string | 是 | minLength=`1`; maxLength=`7`; pattern=`"^#[0-9A-Fa-f]{6}$"` | 不透明sRGB #RRGGBB；不接受CSS表达式/URL |
| `desktop` | string | 是 | minLength=`1`; maxLength=`7`; pattern=`"^#[0-9A-Fa-f]{6}$"` | 不透明sRGB #RRGGBB；不接受CSS表达式/URL |
| `surface` | string | 是 | minLength=`1`; maxLength=`7`; pattern=`"^#[0-9A-Fa-f]{6}$"` | 不透明sRGB #RRGGBB；不接受CSS表达式/URL |
| `text` | string | 是 | minLength=`1`; maxLength=`7`; pattern=`"^#[0-9A-Fa-f]{6}$"` | 不透明sRGB #RRGGBB；不接受CSS表达式/URL |
| `muted_text` | string | 是 | minLength=`1`; maxLength=`7`; pattern=`"^#[0-9A-Fa-f]{6}$"` | 不透明sRGB #RRGGBB；不接受CSS表达式/URL |
| `border` | string | 是 | minLength=`1`; maxLength=`7`; pattern=`"^#[0-9A-Fa-f]{6}$"` | 不透明sRGB #RRGGBB；不接受CSS表达式/URL |
| `danger` | string | 是 | minLength=`1`; maxLength=`7`; pattern=`"^#[0-9A-Fa-f]{6}$"` | 不透明sRGB #RRGGBB；不接受CSS表达式/URL |
| `radius_px` | integer | 是 | minimum=`0`; maximum=`24` | 窗口/面板圆角 |
| `blur_px` | integer | 是 | minimum=`0`; maximum=`32` | 背景模糊强度 |
| `panel_opacity` | number | 是 | minimum=`0.6`; maximum=`1` | 面板不透明度；减少透明度时强制1 |
| `font` | string | 是 | enum=`["system", "sans", "serif", "mono"]` | 宿主内置字体族，不加载外部字体 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "accent",
    "desktop",
    "surface",
    "text",
    "muted_text",
    "border",
    "danger",
    "radius_px",
    "blur_px",
    "panel_opacity",
    "font"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
