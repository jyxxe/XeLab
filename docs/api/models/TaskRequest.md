# TaskRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/TaskRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

任务执行授权会冻结提交时上限

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `workflow_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `input` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `run_as` | string | 否 | default=`"user"`; enum=`["user", "service"]` | 默认 user；service 需要独立批准 |
| `service_account_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | run_as=service 时必填 |
| `timeout_ms` | integer | 否 | minimum=`1000`; maximum=`604800000`; default=`86400000` | 任务整体期限，最长 7 天 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "workflow_id",
    "input"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "run_as": {
            "const": "service"
          }
        },
        "required": [
          "run_as"
        ]
      },
      "then": {
        "required": [
          "service_account_id"
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
