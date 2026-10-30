# OrdinaryExtensionHostRef

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/OrdinaryExtensionHostRef.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

普通OCI宿主的固定部署/安装/镜像摘要

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kind` | JSON | 是 | 见类型定义 |  |
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `installation_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 精确Installation.image_digest，签名OCI来源与清单必须匹配 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "kind",
    "deployment_id",
    "installation_id",
    "installation_digest"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
