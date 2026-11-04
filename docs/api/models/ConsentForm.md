# ConsentForm

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ConsentForm.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

可信同意页原生 URL 编码表单；approved 只接受字符串 true/false。两个同名头字段为传输元数据，验证后移除，不扩展 ConsentDecision JSON。

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `authorization_request_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `approved` | string | 是 | enum=`["true", "false"]` | 明确允许或拒绝固定范围，不接受其他 truthy 表示 |
| `X-CSRF-Token` | string | 否 | minLength=`43`; maxLength=`128` | 与请求头二选一的会话 CSRF 证明 |
| `Idempotency-Key` | string | 否 | minLength=`1`; maxLength=`128` | 与请求头二选一的 24h 去重键 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "authorization_request_id",
    "approved"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
