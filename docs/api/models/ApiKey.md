# ApiKey

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ApiKey.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

密钥元数据；不含明文和摘要

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `owner_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 用途名称 |
| `prefix` | string | 是 | minLength=`1`; maxLength=`32` | 安全显示前缀 |
| `status` | string | 是 | enum=`["active", "revoked", "expired"]` | revoked 不能恢复 |
| `inheritance` | [InheritanceSpec](InheritanceSpec.md) | 是 | 见类型定义 | 用户选择的继承方式；快照上限由内核展开并保存 |
| `conditions` | [CredentialConditions](CredentialConditions.md) | 是 | 见类型定义 | 密钥附加条件，创建者源凭证条件也持续适用 |
| `issuer_credential_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |
| `last_used_at` | string / null | 是 | 见类型定义 | 最近使用时间 |

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
    "owner_id",
    "name",
    "prefix",
    "status",
    "inheritance",
    "conditions",
    "issuer_credential_id",
    "last_used_at"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
