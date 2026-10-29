# OAuthError

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/OAuthError.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

OAuth token 端点使用此错误，不使用平台错误包装

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `error` | string | 是 | enum=`["invalid_request", "invalid_client", "invalid_grant", "unauthorized_client", "unsupported_grant_type", "invalid_scope"]` | OAuth 错误 |
| `error_description` | string | 否 | minLength=`1`; maxLength=`1024` | 无敏感信息的说明 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "error"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
