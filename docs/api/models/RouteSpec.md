# RouteSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/RouteSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

插件路由声明，平台批准后才生效

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `path_prefix` | string | 是 | minLength=`1`; maxLength=`512` | 以 /plugins/{plugin_id}/ 或 /ui/{plugin_id}/ 开头 |
| `methods` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`6` | 方法 |
| `kind` | string | 是 | enum=`["http", "static", "webui"]` | 转发类型 |
| `required_action` | string | 是 | minLength=`1`; maxLength=`160` | 注册的调用权限 |
| `maximum_body_bytes` | integer | 是 | minimum=`0`; maximum=`16777216` | 请求体上限 |
| `timeout_ms` | integer | 是 | minimum=`100`; maximum=`300000` | 代理期限 |
| `http_contract_digest` | string | 否 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `authentication` | string | 否 | default=`"user"`; enum=`["user", "public"]` | public仅允许批准的static/webui GET/HEAD，不公开管理或业务接口 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "path_prefix",
    "methods",
    "kind",
    "required_action",
    "maximum_body_bytes",
    "timeout_ms"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "kind": {
            "const": "http"
          }
        }
      },
      "then": {
        "required": [
          "http_contract_digest"
        ]
      }
    },
    {
      "if": {
        "properties": {
          "authentication": {
            "const": "public"
          }
        },
        "required": [
          "authentication"
        ]
      },
      "then": {
        "properties": {
          "kind": {
            "enum": [
              "static",
              "webui"
            ]
          },
          "methods": {
            "items": {
              "enum": [
                "GET",
                "HEAD"
              ]
            }
          }
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
