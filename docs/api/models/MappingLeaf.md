# MappingLeaf

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MappingLeaf.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

输入映射叶子，三个分支严格互斥

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `literal` | [JsonValue](JsonValue.md) | 否 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `task_input` | string | 否 | minLength=`0`; maxLength=`1024` | RFC6901 Pointer；空字符串表示整个输入 |
| `step_output` | object | 否 | 见类型定义 |  |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [],
  "oneOf": [
    {
      "required": [
        "literal"
      ]
    },
    {
      "required": [
        "task_input"
      ]
    },
    {
      "required": [
        "step_output"
      ]
    }
  ],
  "maxProperties": 1
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
