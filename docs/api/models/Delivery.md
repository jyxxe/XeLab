# Delivery

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Delivery.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

至少一次事件投递

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `delivery_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `subscription_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `event` | [Event](Event.md) | 是 | 见类型定义 | 持久化事件信封 |
| `attempt` | integer | 是 | minimum=`1`; maximum=`10` | 本次序号 |
| `lease_token` | string | 是 | minLength=`1`; maxLength=`512` | 确认租约令牌，不可写入普通日志 |
| `lease_expires_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 确认期限，默认 30s |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "delivery_id",
    "subscription_id",
    "event",
    "attempt",
    "lease_token",
    "lease_expires_at"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
