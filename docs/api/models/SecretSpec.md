# SecretSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SecretSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

创建平台托管密钥；仅首次写入，不提供用户明文读取端点

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 名称 |
| `value_base64` | string | 是 | minLength=`1`; maxLength=`87384`; writeOnly=`true` | Base64 编码内容，解码后最多 64 KiB |
| `expires_at` | string | 否 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 外部凭证过期时间 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "value_base64"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
