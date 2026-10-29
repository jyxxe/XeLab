# OAuthTokenRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/OAuthTokenRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

OAuth 表单：授权码+PKCE或刷新凭证

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `grant_type` | string | 是 | enum=`["authorization_code", "refresh_token"]` | 授权模式 |
| `client_id` | string | 是 | minLength=`1`; maxLength=`128` | 注册客户端 |
| `code` | string | 否 | minLength=`1`; maxLength=`512` | 一次性授权码 |
| `redirect_uri` | string | 否 | format=`"uri"`; minLength=`1`; maxLength=`2048` | 必须与注册和授权请求一致 |
| `code_verifier` | string | 否 | minLength=`43`; maxLength=`128`; pattern=`"^[A-Za-z0-9._~-]{43,128}$"` | PKCE verifier，43–128 字符 |
| `refresh_token` | string | 否 | minLength=`1`; maxLength=`512` | 刷新凭证；轮换并检测重用 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "grant_type",
    "client_id"
  ],
  "oneOf": [
    {
      "properties": {
        "grant_type": {
          "const": "authorization_code"
        }
      },
      "required": [
        "code",
        "redirect_uri",
        "code_verifier"
      ],
      "not": {
        "required": [
          "refresh_token"
        ]
      }
    },
    {
      "properties": {
        "grant_type": {
          "const": "refresh_token"
        }
      },
      "required": [
        "refresh_token"
      ],
      "not": {
        "anyOf": [
          {
            "required": [
              "code"
            ]
          },
          {
            "required": [
              "redirect_uri"
            ]
          },
          {
            "required": [
              "code_verifier"
            ]
          }
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
