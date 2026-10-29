# PermissionRule

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/PermissionRule.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

动作与对象选择器共同定义授权

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `action` | string | 是 | minLength=`1`; maxLength=`160`; pattern=`"^[a-z][a-z0-9._-]+$"` | 必须在权限目录注册，区分大小写 |
| `resource` | [ResourceSelector](ResourceSelector.md) | 是 | 见类型定义 | 资源选择器；all 仅表示指定类型内所有资源 |
| `conditions` | [RuleConditions](RuleConditions.md) | 否 | 见类型定义 | 规则附加约束；省略表示没有该项约束 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "action",
    "resource"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
