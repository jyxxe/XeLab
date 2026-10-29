# MicroCallDiagnostics

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroCallDiagnostics.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

单次微调用诊断，不作为公共日志或业务输出

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `call_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_plugin_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `package_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `elapsed_ms` | integer | 是 | minimum=`0`; maximum=`30000` | 运行耗时 |
| `peak_heap_bytes` | integer | 是 | minimum=`0`; maximum=`67108864` | VM观测堆峰值 |
| `stderr` | string | 是 | minLength=`0`; maxLength=`65536` | 有界原始诊断，单独授权，日志不得包含凭证 |
| `stderr_truncated` | boolean | 是 | 见类型定义 |  |
| `expires_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "call_id",
    "micro_plugin_id",
    "micro_version_id",
    "micro_deployment_id",
    "package_digest",
    "elapsed_ms",
    "peak_heap_bytes",
    "stderr",
    "stderr_truncated",
    "expires_at"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
