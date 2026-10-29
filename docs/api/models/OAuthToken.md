# OAuthToken

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/OAuthToken.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

标准 OAuth token 响应；访问令牌绑定 subject/client/audience

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `access_token` | string | 是 | minLength=`1`; maxLength=`4096` | Bearer 令牌 |
| `token_type` | string | 是 | enum=`["Bearer"]` | 类型 |
| `expires_in` | integer | 是 | minimum=`900`; maximum=`900` | 秒，固定 900 |
| `refresh_token` | string | 否 | minLength=`1`; maxLength=`512` | 可选轮换凭证，最长 24h |
| `scope` | string | 是 | minLength=`1`; maxLength=`512` | 平台授权范围句柄；实际规则保存在内核 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "access_token",
    "token_type",
    "expires_in",
    "scope"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
