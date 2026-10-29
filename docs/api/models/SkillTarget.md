# SkillTarget

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillTarget.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

普通capability/http目标或第一类micro目标；micro只固定核心微版本/微部署/制品，不接受即时源码或任意URL

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kind` | string | 是 | enum=`["capability", "http", "micro"]` | 调用类型 |
| `plugin_id` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$"` | 插件稳定 ID；不含版本 |
| `contract` | [ContractRef](ContractRef.md) | 否 | 见类型定义 | 不可变能力契约引用 |
| `http_operation_id` | string | 否 | minLength=`1`; maxLength=`128` | 插件 OpenAPI 中的精确 operationId |
| `http_contract_digest` | string | 否 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `micro_version_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_deployment_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `package_digest` | string | 否 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "kind",
    "plugin_id"
  ],
  "oneOf": [
    {
      "properties": {
        "kind": {
          "const": "capability"
        }
      },
      "required": [
        "contract"
      ],
      "not": {
        "anyOf": [
          {
            "required": [
              "http_operation_id"
            ]
          },
          {
            "required": [
              "http_contract_digest"
            ]
          },
          {
            "required": [
              "micro_version_id"
            ]
          },
          {
            "required": [
              "micro_deployment_id"
            ]
          },
          {
            "required": [
              "package_digest"
            ]
          }
        ]
      }
    },
    {
      "properties": {
        "kind": {
          "const": "http"
        }
      },
      "required": [
        "http_operation_id",
        "http_contract_digest"
      ],
      "not": {
        "anyOf": [
          {
            "required": [
              "contract"
            ]
          },
          {
            "required": [
              "micro_version_id"
            ]
          },
          {
            "required": [
              "micro_deployment_id"
            ]
          },
          {
            "required": [
              "package_digest"
            ]
          }
        ]
      }
    },
    {
      "properties": {
        "kind": {
          "const": "micro"
        }
      },
      "required": [
        "micro_version_id",
        "micro_deployment_id",
        "package_digest"
      ],
      "not": {
        "anyOf": [
          {
            "required": [
              "contract"
            ]
          },
          {
            "required": [
              "http_operation_id"
            ]
          },
          {
            "required": [
              "http_contract_digest"
            ]
          }
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
