# Session

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Session.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

会话元数据，不返回校验摘要

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `account_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `client_id` | string | 是 | minLength=`1`; maxLength=`128` | 可信客户端 ID |
| `expires_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 绝对过期时间 |
| `last_seen_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近验证时间 |
| `status` | string | 是 | enum=`["active", "revoked", "expired"]` | 会话状态 |

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
    "account_id",
    "client_id",
    "expires_at",
    "last_seen_at",
    "status"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
