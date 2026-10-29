# CapabilityProvider

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CapabilityProvider.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

可路由能力提供者；无宿主内部地址

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `instance_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `availability` | string | 是 | enum=`["ready", "degraded", "unavailable"]` | 逐能力就绪状态 |
| `features` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`256` | 功能特性 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "contract",
    "deployment_id",
    "instance_id",
    "availability",
    "features"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
