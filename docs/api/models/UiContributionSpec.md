# UiContributionSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiContributionSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

可选独立iframe窗口或桌面组件；两种均不得将插件JS注入桌面壳

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | minLength=`1`; maxLength=`64`; pattern=`"^[a-z][a-z0-9_-]{0,63}$"` | 插件内稳定标识 |
| `kind` | string | 是 | enum=`["window", "widget"]` | 显示形态 |
| `title` | string | 是 | minLength=`1`; maxLength=`80` | 作为普通文本渲染的名称 |
| `description` | string | 是 | minLength=`1`; maxLength=`512` | 用途 |
| `entry_path` | string | 是 | minLength=`1`; maxLength=`256`; pattern=`"^/[A-Za-z0-9_./-]*$"` | 相对插件UI根的绝对路径；拒绝..、反斜线、编码逃逸、query/fragment、URL与双斜线 |
| `icon_path` | string | 否 | minLength=`1`; maxLength=`256`; pattern=`"^/[A-Za-z0-9_./-]*$"` | 相对插件UI根的绝对路径；拒绝..、反斜线、编码逃逸、query/fragment、URL与双斜线 |
| `skill_bindings` | array&lt;[UiSkillBinding](UiSkillBinding.md)&gt; | 是 | minItems=`0`; maxItems=`32` | 只是桥接白名单申请，不授予任何权限 |
| `window` | [UiWindowSize](UiWindowSize.md) | 否 | 见类型定义 | 窗口逻辑像素约束，min<=default<=max；小屏由宿主裁剪到可用工作区 |
| `widget` | [UiWidgetSize](UiWidgetSize.md) | 否 | 见类型定义 | 组件占用桌面网格，1格=96逻辑像素，间距12；不控制全局桌面 |
| `refresh_interval_seconds` | integer | 否 | minimum=`0`; maximum=`3600`; default=`0` | 组件可见时刷新间隔；0关闭，非零必须>=30；窗口固定0 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "kind",
    "title",
    "description",
    "entry_path",
    "skill_bindings"
  ],
  "oneOf": [
    {
      "properties": {
        "kind": {
          "const": "window"
        },
        "refresh_interval_seconds": {
          "const": 0
        }
      },
      "required": [
        "window"
      ],
      "not": {
        "required": [
          "widget"
        ]
      }
    },
    {
      "properties": {
        "kind": {
          "const": "widget"
        },
        "refresh_interval_seconds": {
          "anyOf": [
            {
              "const": 0
            },
            {
              "type": "integer",
              "minimum": 30,
              "maximum": 3600
            }
          ]
        }
      },
      "required": [
        "widget"
      ],
      "not": {
        "required": [
          "window"
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
