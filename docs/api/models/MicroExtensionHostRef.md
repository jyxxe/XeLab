# MicroExtensionHostRef

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroExtensionHostRef.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

微宿主精确部署/版本/微包；物理根由其固定MPR绑定解析

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kind` | JSON | 是 | 见类型定义 |  |
| `micro_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `package_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "kind",
    "micro_deployment_id",
    "micro_version_id",
    "package_digest"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
