# MicroBusinessError

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroBusinessError.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

微制品声明的业务错误；不得使用平台保留码或认证状态

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `code` | string | 是 | minLength=`1`; maxLength=`80`; pattern=`"^[A-Z][A-Z0-9_]{0,79}$"` | 平台保留错误码之外的稳定业务码 |
| `http_status` | integer | 是 | enum=`[400, 409, 413, 429, 502, 503, 504]` |  |
| `retryable` | boolean | 是 | default=`false` |  |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "code",
    "http_status",
    "retryable"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
