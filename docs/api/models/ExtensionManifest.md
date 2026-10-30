# ExtensionManifest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ExtensionManifest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

独立扩展制品清单；contribution.json为有界严格JSON，不是OCI/可执行JS；旧宿主必须显式发布匹配扩展点

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `format_version` | JSON | 是 | 见类型定义 |  |
| `version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `description` | string | 是 | minLength=`1`; maxLength=`2048` | 扩展用途 |
| `host_kind` | string | 是 | enum=`["ordinary", "micro", "extension"]` |  |
| `point_id` | string | 是 | minLength=`1`; maxLength=`80`; pattern=`"^[a-z][a-z0-9.-]+$"` | 精确匹配宿主扩展点 |
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `mode` | string | 是 | enum=`["data", "skill"]` |  |
| `handler_skill_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `handler_definition_digest` | string | 否 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `extension_points` | array&lt;[ExtensionPoint](ExtensionPoint.md)&gt; | 否 | minItems=`0`; maxItems=`8` |  |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "format_version",
    "version",
    "description",
    "host_kind",
    "point_id",
    "contract",
    "mode"
  ],
  "additionalProperties": false,
  "allOf": [
    {
      "if": {
        "properties": {
          "mode": {
            "const": "skill"
          }
        }
      },
      "then": {
        "required": [
          "handler_skill_id",
          "handler_definition_digest"
        ]
      },
      "else": {
        "not": {
          "anyOf": [
            {
              "required": [
                "handler_skill_id"
              ]
            },
            {
              "required": [
                "handler_definition_digest"
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
