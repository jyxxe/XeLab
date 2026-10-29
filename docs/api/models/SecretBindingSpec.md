# SecretBindingSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SecretBindingSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

仅批准指定部署使用指定密钥版本；不授予明文读取给用户

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `secret_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `maximum_lease_seconds` | integer | 是 | minimum=`1`; maximum=`900`; default=`300` | 租约时长；不能撤回插件已经获知的原值 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "deployment_id",
    "secret_id",
    "maximum_lease_seconds"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
