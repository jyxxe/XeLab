# SkillBatchInputItem

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillBatchInputItem.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

一个有界 Skill 调用单元；id 在本批内唯一

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | minLength=`1`; maxLength=`80`; pattern=`"^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$"` | 调用方关联键；不是行ID或授权凭证 |
| `input` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "input"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
