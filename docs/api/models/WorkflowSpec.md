# WorkflowSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/WorkflowSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

不可变 DAG 版本；最多 64 步

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 名称 |
| `input_schema` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `steps` | array&lt;[WorkflowStep](WorkflowStep.md)&gt; | 是 | minItems=`1`; maxItems=`64` | 节点 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "input_schema",
    "steps"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
