# UiThemeSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiThemeSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

系统/明亮/深色或自定义令牌；系统模式使用浏览器偏好

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `mode` | string | 是 | default=`"system"`; enum=`["system", "light", "dark", "custom"]` | 主题模式 |
| `custom_tokens` | [UiThemeTokens](UiThemeTokens.md) | 否 | 见类型定义 | 有限主题令牌；组件使用角色令牌，禁止传入任意CSS/HTML/脚本 |
| `reduce_motion` | boolean | 是 | default=`false` | 叠加系统偏好，任一要求时减少动画 |
| `reduce_transparency` | boolean | 是 | default=`false` | 以实色面板替代模糊透明 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "mode",
    "reduce_motion",
    "reduce_transparency"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "mode": {
            "const": "custom"
          }
        },
        "required": [
          "mode"
        ]
      },
      "then": {
        "required": [
          "custom_tokens"
        ]
      },
      "else": {
        "not": {
          "required": [
            "custom_tokens"
          ]
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
