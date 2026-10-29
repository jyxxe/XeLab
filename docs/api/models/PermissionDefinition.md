# PermissionDefinition

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/PermissionDefinition.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

权限注册项

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `action` | string | 是 | minLength=`1`; maxLength=`160` | 动作名称 |
| `resource_kinds` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`8` | 动作支持的对象类型 |
| `description` | string | 是 | minLength=`1`; maxLength=`1024` | 行为说明 |
| `self_service` | boolean | 是 | 见类型定义 | 是否属于所有活跃用户的自助能力 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "action",
    "resource_kinds",
    "description",
    "self_service"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
