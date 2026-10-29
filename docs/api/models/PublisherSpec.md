# PublisherSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/PublisherSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

添加发布者信任根

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`128` | 发布者名称 |
| `key_id` | string | 是 | minLength=`1`; maxLength=`128` | 签名密钥标识 |
| `public_key_pem` | string | 是 | minLength=`1`; maxLength=`4096` | Ed25519 PEM 公钥 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "key_id",
    "public_key_pem"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
