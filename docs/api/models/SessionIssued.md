# SessionIssued

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SessionIssued.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

登录创建会话；浏览器同时设置内核来源的安全 Cookie

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `session` | [Session](Session.md) | 是 | 见类型定义 | 会话元数据，不返回校验摘要 |
| `access_token` | string | 是 | minLength=`1`; maxLength=`512` | 不透明 Bearer 会话凭证；只返回一次 |
| `token_type` | string | 是 | enum=`["Bearer"]` | 认证类型 |
| `expires_in` | integer | 是 | minimum=`1`; maximum=`86400` | 秒；默认 3600 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "session",
    "access_token",
    "token_type",
    "expires_in"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
