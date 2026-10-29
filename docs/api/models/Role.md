# Role

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Role.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

内置不可变角色模板；scope 在绑定时提供

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | enum=`["Owner", "Operator", "Member", "Auditor", "Guest"]` | 角色名称 |
| `actions` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`256` | 默认动作 |
| `description` | string | 是 | minLength=`1`; maxLength=`1024` | 角色职责 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "actions",
    "description"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
