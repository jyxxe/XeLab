# SubscriptionSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SubscriptionSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

拉取式事件订阅；禁止任意外部 webhook 目标

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `topic` | string | 是 | minLength=`1`; maxLength=`160` | 精确主题，不支持通配符 |
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 订阅名称 |
| `start` | string | 否 | default=`"latest"`; enum=`["latest", "earliest"]` | 起点 |
| `maximum_attempts` | integer | 否 | minimum=`1`; maximum=`10`; default=`5` | 投递次数上限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "topic",
    "name"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
