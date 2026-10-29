# AuditEvent

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/AuditEvent.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

追加式审计，禁止更新和普通删除

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `subject_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |
| `actor_instance_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |
| `credential_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |
| `delegation_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |
| `request_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `action` | string | 是 | minLength=`1`; maxLength=`160` | 动作 |
| `resource` | [ResourceSelector](ResourceSelector.md) | 是 | 见类型定义 | 资源选择器；all 仅表示指定类型内所有资源 |
| `outcome` | string | 是 | enum=`["allowed", "denied", "succeeded", "failed"]` | 授权/执行结果 |
| `policy_revision` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | 策略版本 |
| `reason_code` | string | 是 | minLength=`1`; maxLength=`80` | 稳定原因码 |

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
    "subject_id",
    "actor_instance_id",
    "credential_id",
    "delegation_id",
    "request_id",
    "action",
    "resource",
    "outcome",
    "policy_revision",
    "reason_code"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
