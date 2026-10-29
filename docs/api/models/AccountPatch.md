# AccountPatch

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/AccountPatch.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

仅修改显示名称或启用状态；禁止修改 parent_id、kind 和根标记

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `display_name` | string | 否 | minLength=`1`; maxLength=`80` | 新的显示名称 |
| `status` | string | 否 | enum=`["active", "disabled"]` | 恢复 active 仍需祖先有效 |
| `expires_at` | string / null | 否 | 见类型定义 | 由合法父级/账户管理员调整，不能突破源凭证链；本人资料修改权限不包含此项 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [],
  "minProperties": 1
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
