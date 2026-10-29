# UiPreferencesSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiPreferencesSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

当前账户跨登录保留的非敏感偏好；不允许存令牌/业务输入/结果

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `theme` | [UiThemeSpec](UiThemeSpec.md) | 是 | 见类型定义 | 系统/明亮/深色或自定义令牌；系统模式使用浏览器偏好 |
| `dock` | [UiDockSpec](UiDockSpec.md) | 是 | 见类型定义 | 应用Dock偏好；pinned中同贡献不得重复 |
| `widgets` | array&lt;[UiWidgetPlacement](UiWidgetPlacement.md)&gt; | 是 | minItems=`0`; maxItems=`32` | 桌面组件布局；初始为空，不默认后台运行 |
| `restore_windows` | boolean | 是 | default=`true` | 是否恢复本浏览器中同用户窗口位置；不自动重放调用 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "theme",
    "dock",
    "widgets",
    "restore_windows"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
