# SkillBatchResolution

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillBatchResolution.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

人工核实unknown项目；需要单独batch.resolve和证据，禁止直接重放

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `resolution` | string | 是 | enum=`["confirmed_succeeded", "confirmed_not_executed"]` | 外部证据核实结果 |
| `evidence` | string | 是 | minLength=`1`; maxLength=`2048` | 审计依据，禁止填凭证/秘密 |
| `output` | [JsonValue](JsonValue.md) | 否 | 见类型定义 | 确认成功时必填，按原Skill.output_schema校验；核心构造结果封装 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "resolution",
    "evidence"
  ],
  "oneOf": [
    {
      "properties": {
        "resolution": {
          "const": "confirmed_succeeded"
        }
      },
      "required": [
        "output"
      ]
    },
    {
      "properties": {
        "resolution": {
          "const": "confirmed_not_executed"
        }
      },
      "not": {
        "required": [
          "output"
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
