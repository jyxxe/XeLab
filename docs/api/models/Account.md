# Account

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Account.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

用户或服务账户；API Key 不属于账户

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `parent_id` | string / null | 是 | 见类型定义 | 父账户；独立根账户为空 |
| `kind` | string | 是 | enum=`["user", "service"]` | 主体类型 |
| `display_name` | string | 是 | minLength=`1`; maxLength=`80` | 显示名称 |
| `status` | string | 是 | enum=`["pending", "active", "disabled", "archived"]` | 归档为逻辑删除，不解除继承链 |
| `independent_owner` | boolean | 是 | 见类型定义 | 仅宿主可信初始化/恢复可建立的恢复根标记 |
| `authz_revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 授权版本 |
| `inheritance` | [InheritanceSpec](InheritanceSpec.md) / null | 是 | 见类型定义 | 可为空 |
| `expires_at` | string / null | 是 | 见类型定义 | 可为空；仍受源链有效期限制 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "parent_id",
    "kind",
    "display_name",
    "status",
    "independent_owner",
    "authz_revision",
    "inheritance",
    "expires_at"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
