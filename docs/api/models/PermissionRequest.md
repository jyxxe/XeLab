# PermissionRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/PermissionRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

申请权限与必需性

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `rule` | [PermissionRule](PermissionRule.md) | 是 | 见类型定义 | 动作与对象选择器共同定义授权 |
| `required` | boolean | 是 | default=`true` | 为 true 时未批准则不能启用相关能力 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "rule",
    "required"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
