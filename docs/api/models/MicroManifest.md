# MicroManifest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroManifest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

核心独立保存的不可变微制品清单；不是OCI Manifest

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `format_version` | JSON | 是 | 见类型定义 |  |
| `version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `description` | string | 是 | minLength=`1`; maxLength=`2048` | 用途 |
| `engine` | JSON | 是 | 见类型定义 |  |
| `protocol` | JSON | 是 | 见类型定义 |  |
| `entrypoint` | JSON | 是 | 见类型定义 |  |
| `input_schema` | [MicroBusinessSchema](MicroBusinessSchema.md) | 是 | 见类型定义 | 有界纯数据Schema；节点/深度/字节/求值预算由核心及运行时再次检查 |
| `output_schema` | [MicroBusinessSchema](MicroBusinessSchema.md) | 是 | 见类型定义 | 有界纯数据Schema；节点/深度/字节/求值预算由核心及运行时再次检查 |
| `limits` | [MicroExecutionLimits](MicroExecutionLimits.md) | 是 | 见类型定义 | 获批执行预算，必须同时不超过版本声明和MPR上限 |
| `side_effect` | string | 是 | enum=`["none", "non_repeatable"]` | 纯计算为none；HTTP转发可能计费，必须non_repeatable |
| `execution_profile` | string | 否 | default=`"pure-json-v1"`; enum=`["pure-json-v1", "controlled-http-v1"]` | 省略保留纯计算；HTTP需新制品和显式绑定/审批 |
| `http_endpoints` | array&lt;[MicroHttpEndpoint](MicroHttpEndpoint.md)&gt; | 否 | minItems=`0`; maxItems=`8` | 别名唯一；仅声明可经核心代理使用的固定模板 |
| `business_errors` | array&lt;[MicroBusinessError](MicroBusinessError.md)&gt; | 否 | minItems=`0`; maxItems=`32` | host.fail只允许报告已声明码，不得伪造平台认证/授权失败 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "format_version",
    "version",
    "description",
    "engine",
    "protocol",
    "entrypoint",
    "input_schema",
    "output_schema",
    "limits",
    "side_effect"
  ],
  "additionalProperties": false,
  "allOf": [
    {
      "if": {
        "properties": {
          "execution_profile": {
            "const": "controlled-http-v1"
          }
        },
        "required": [
          "execution_profile"
        ]
      },
      "then": {
        "required": [
          "http_endpoints"
        ],
        "properties": {
          "side_effect": {
            "const": "non_repeatable"
          },
          "http_endpoints": {
            "minItems": 1
          }
        }
      },
      "else": {
        "properties": {
          "side_effect": {
            "const": "none"
          },
          "http_endpoints": {
            "maxItems": 0
          },
          "business_errors": {
            "maxItems": 0
          }
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
