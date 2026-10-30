# ResourceSelector

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ResourceSelector.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

资源选择器；all 仅表示指定类型内所有资源

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kind` | string | 是 | enum=`["platform", "account", "group", "credential", "plugin", "deployment", "instance", "capability", "task", "workflow", "schedule", "volume", "secret", "blob", "event", "subscription", "client", "publisher", "backup", "operation", "audit", "quota", "model", "call", "skill", "skill_batch", "micro_plugin", "extension_plugin"]` | 资源类型 |
| `match` | string | 是 | enum=`["all", "ids", "owned_by"]` | 匹配方式 |
| `ids` | array&lt;string&gt; | 否 | minItems=`1`; maxItems=`256` | 显式资源标识 |
| `owner_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | owned_by 的固定账户标识 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "kind",
    "match"
  ],
  "oneOf": [
    {
      "properties": {
        "match": {
          "const": "all"
        }
      },
      "not": {
        "anyOf": [
          {
            "required": [
              "ids"
            ]
          },
          {
            "required": [
              "owner_id"
            ]
          }
        ]
      }
    },
    {
      "properties": {
        "match": {
          "const": "ids"
        }
      },
      "required": [
        "ids"
      ],
      "not": {
        "required": [
          "owner_id"
        ]
      }
    },
    {
      "properties": {
        "match": {
          "const": "owned_by"
        }
      },
      "required": [
        "owner_id"
      ],
      "not": {
        "required": [
          "ids"
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
