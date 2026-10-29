# PublishMicroVersion

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/PublishMicroVersion.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

从当前获准Blob复制源码到核心不可变存储，不依赖源BlobTTL

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `manifest` | [MicroManifest](MicroManifest.md) | 是 | 见类型定义 | 核心独立保存的不可变微制品清单；不是OCI Manifest |
| `source_blob_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 需当前blob.read；源码<=256KiB UTF8且无NUL/BOM |
| `source_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 原始源码字节SHA-256，不规范化代码 |
| `runtime_installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 精确已安装且声明Manifest.micro_runtime的OCI制品 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "manifest",
    "source_blob_id",
    "source_digest",
    "runtime_installation_id"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
