# EffectivePermissions

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/EffectivePermissions.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

某个账户当前有效权限；不返回其他账户私有信息

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `account_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `authz_revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 当前版本 |
| `rules` | array&lt;[PermissionRule](PermissionRule.md)&gt; | 是 | minItems=`0`; maxItems=`4096` | 展开、求交后的规则 |
| `blocked_by` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`256` | 造成失效的授权链节点 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "account_id",
    "authz_revision",
    "rules",
    "blocked_by"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
