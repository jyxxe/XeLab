# PublishExtensionVersion

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/PublishExtensionVersion.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

发布独立不可变清单与贡献；package_digest=SHA256(JCS({manifest,source_digest}))，不执行贡献或自动注册Skill

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `manifest` | [ExtensionManifest](ExtensionManifest.md) | 是 | 见类型定义 | 独立扩展制品清单；contribution.json为有界严格JSON，不是OCI/可执行JS；旧宿主必须显式发布匹配扩展点 |
| `source_blob_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 当前blob.read获准的contribution.json，<=256KiB，复制到核心不可变存储，不依赖Blob TTL |
| `source_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "manifest",
    "source_blob_id",
    "source_digest"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
