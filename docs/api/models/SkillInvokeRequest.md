# SkillInvokeRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillInvokeRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

目标由 Skill 固定，调用方不能覆盖 plugin/contract/address

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `input` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `timeout_ms` | integer | 否 | minimum=`100`; maximum=`300000`; default=`120000` | 显式值不超过Skill.maximum_timeout_ms；省略取min(120000,Skill上限) |
| `idempotency_key` | string | 否 | minLength=`1`; maxLength=`128` | 有副作用调用的稳定业务去重键 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "input"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
