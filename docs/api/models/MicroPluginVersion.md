# MicroPluginVersion

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroPluginVersion.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

发布后不可变；元数据验证不等于脚本已Prepare成功

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_plugin_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `manifest` | [MicroManifest](MicroManifest.md) | 是 | 见类型定义 | 核心独立保存的不可变微制品清单；不是OCI Manifest |
| `source_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `source_bytes` | integer | 是 | minimum=`1`; maximum=`262144` | 源码字节数 |
| `package_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `runtime_installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "id",
    "micro_plugin_id",
    "created_at",
    "manifest",
    "source_digest",
    "source_bytes",
    "package_digest",
    "runtime_installation_id"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
