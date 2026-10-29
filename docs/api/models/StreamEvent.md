# StreamEvent

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/StreamEvent.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

SSE data 对象；terminal 只出现一次

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `call_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `sequence` | integer | 是 | minimum=`1`; maximum=`9007199254740991` | 从 1 开始单调递增 |
| `kind` | string | 是 | enum=`["accepted", "item", "completed", "failed", "canceled"]` | 事件类型 |
| `payload` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `terminal` | boolean | 是 | 见类型定义 | 结束事件标记 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "call_id",
    "sequence",
    "kind",
    "payload",
    "terminal"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "kind": {
            "enum": [
              "accepted",
              "item"
            ]
          }
        }
      },
      "then": {
        "properties": {
          "terminal": {
            "const": false
          }
        }
      }
    },
    {
      "if": {
        "properties": {
          "kind": {
            "enum": [
              "completed",
              "failed",
              "canceled"
            ]
          }
        }
      },
      "then": {
        "properties": {
          "terminal": {
            "const": true
          }
        }
      }
    },
    {
      "if": {
        "properties": {
          "kind": {
            "const": "accepted"
          }
        }
      },
      "then": {
        "properties": {
          "payload": {
            "type": "object",
            "additionalProperties": false
          }
        }
      }
    },
    {
      "if": {
        "properties": {
          "kind": {
            "const": "completed"
          }
        }
      },
      "then": {
        "properties": {
          "payload": {
            "$ref": "#/components/schemas/CallResult"
          }
        }
      }
    },
    {
      "if": {
        "properties": {
          "kind": {
            "const": "failed"
          }
        }
      },
      "then": {
        "properties": {
          "payload": {
            "$ref": "#/components/schemas/ErrorDetail"
          }
        }
      }
    },
    {
      "if": {
        "properties": {
          "kind": {
            "const": "canceled"
          }
        }
      },
      "then": {
        "properties": {
          "payload": {
            "type": "object",
            "required": [
              "reason"
            ],
            "additionalProperties": false,
            "properties": {
              "reason": {
                "enum": [
                  "requested",
                  "deadline",
                  "authorization",
                  "disconnected"
                ]
              }
            }
          }
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
