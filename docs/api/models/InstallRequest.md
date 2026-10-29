# InstallRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/InstallRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

安装不可变镜像；验证签名、清单及契约，不自动启用

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `image` | string | 是 | minLength=`1`; maxLength=`2048`; pattern=`"^.+@sha256:[a-f0-9]{64}$"` | OCI 仓库引用，必须固定 @sha256 摘要 |
| `publisher_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `signature` | string | 是 | minLength=`1`; maxLength=`512` | 对签名信封的 Base64 Ed25519 签名；信封见制品文档 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "image",
    "publisher_id",
    "signature"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
