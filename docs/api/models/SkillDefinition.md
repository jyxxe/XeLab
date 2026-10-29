# SkillDefinition

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillDefinition.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

不可变的核心转发声明，不是可执行脚本或 Codex SKILL.md

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`160`; pattern=`"^[a-z][a-z0-9.-]+$"` | 插件命名空间内唯一名称，plugin_id 前缀 |
| `version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `description` | string | 是 | minLength=`1`; maxLength=`2048` | 业务用途 |
| `input_schema` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 目标业务输入Schema；micro与MicroManifest精确相等，不等同运行时RPC控制封装 |
| `output_schema` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 必须与目标响应一致 |
| `stream_item_schema` | [JsonValue](JsonValue.md) / null | 是 | 见类型定义 |  |
| `target` | [SkillTarget](SkillTarget.md) | 是 | 见类型定义 | 普通capability/http目标或第一类micro目标；micro只固定核心微版本/微部署/制品，不接受即时源码或任意URL |
| `mapping` | string | 是 | default=`"identity"`; enum=`["identity"]` | 首版仅支持原样传入传出 |
| `maximum_timeout_ms` | integer | 是 | minimum=`100`; maximum=`300000`; default=`120000` | 含排队、转发、推理的总期限 |
| `input_blobs` | boolean | 是 | default=`false` | 根据输入 Schema 的资源标记生成本次 blob.read 精确白名单 |
| `tags` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`32` | 如 embedding/text/multimodal |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "version",
    "description",
    "input_schema",
    "output_schema",
    "stream_item_schema",
    "target",
    "mapping",
    "maximum_timeout_ms",
    "input_blobs",
    "tags"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "target": {
            "properties": {
              "kind": {
                "const": "micro"
              }
            },
            "required": [
              "kind"
            ]
          }
        },
        "required": [
          "target"
        ]
      },
      "then": {
        "properties": {
          "stream_item_schema": {
            "type": "null"
          },
          "input_blobs": {
            "const": false
          },
          "maximum_timeout_ms": {
            "maximum": 30000
          },
          "input_schema": {
            "$ref": "#/components/schemas/MicroBusinessSchema"
          },
          "output_schema": {
            "$ref": "#/components/schemas/MicroBusinessSchema"
          }
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
