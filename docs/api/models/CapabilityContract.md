# CapabilityContract

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CapabilityContract.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

能力契约；JSON Schema 2020-12 文档与注册制品摘要匹配

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `input_schema` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `output_schema` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `stream_item_schema` | [JsonValue](JsonValue.md) / null | 是 | 见类型定义 | 可为空 |
| `side_effect` | string | 是 | enum=`["none", "idempotent", "queryable", "non_repeatable"]` | 决定重试与恢复策略 |
| `maximum_input_bytes` | integer | 是 | minimum=`1`; maximum=`4194304` | 序列化 JSON 最大字节 |
| `maximum_output_bytes` | integer | 是 | minimum=`1`; maximum=`16777216` | 单次输出上限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "contract",
    "input_schema",
    "output_schema",
    "stream_item_schema",
    "side_effect",
    "maximum_input_bytes",
    "maximum_output_bytes"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
