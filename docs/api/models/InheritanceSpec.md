# InheritanceSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/InheritanceSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

用户选择的继承方式；快照上限由内核展开并保存

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `mode` | string | 是 | enum=`["selected", "all_snapshot", "follow_parent"]` | selected：固定选定上限；all_snapshot：创建时全部；follow_parent：持续全量 |
| `rules` | array&lt;[PermissionRule](PermissionRule.md)&gt; | 否 | minItems=`0`; maxItems=`256` | selected 直接选择的规则 |
| `group_versions` | array&lt;[GroupVersionRef](GroupVersionRef.md)&gt; | 否 | minItems=`0`; maxItems=`256` | selected 选择的组版本 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "mode"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "mode": {
            "const": "selected"
          }
        }
      },
      "then": {
        "anyOf": [
          {
            "required": [
              "rules"
            ],
            "properties": {
              "rules": {
                "minItems": 1
              }
            }
          },
          {
            "required": [
              "group_versions"
            ],
            "properties": {
              "group_versions": {
                "minItems": 1
              }
            }
          }
        ]
      }
    },
    {
      "if": {
        "properties": {
          "mode": {
            "enum": [
              "all_snapshot",
              "follow_parent"
            ]
          }
        }
      },
      "then": {
        "not": {
          "anyOf": [
            {
              "required": [
                "rules"
              ]
            },
            {
              "required": [
                "group_versions"
              ]
            }
          ]
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
