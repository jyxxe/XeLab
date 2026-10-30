# NestedExtensionHostRef

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/NestedExtensionHostRef.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

扩展作为宿主；须沿固定祖先链到真实ordinary或micro根，不接受回指或循环

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kind` | JSON | 是 | 见类型定义 |  |
| `extension_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `extension_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `package_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "kind",
    "extension_deployment_id",
    "extension_version_id",
    "package_digest"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
