# AckDelivery

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/AckDelivery.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

只接受当前订阅与当前有效租约

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `delivery_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `lease_token` | string | 是 | minLength=`1`; maxLength=`512` | 投递租约凭证 |
| `outcome` | string | 是 | enum=`["ack", "retry", "dead_letter"]` | ack 推进游标，retry 退避，dead_letter 停止本消息 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "delivery_id",
    "lease_token",
    "outcome"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
