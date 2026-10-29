# CallStatus

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CallStatus.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

调用状态与结果查询

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `call_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `state` | string | 是 | enum=`["accepted", "running", "succeeded", "failed", "canceled", "unknown"]` | unknown 不能直接视为失败重试 |
| `result` | [CallResult](CallResult.md) / null | 是 | 见类型定义 | 可为空 |
| `error` | [ErrorDetail](ErrorDetail.md) / null | 是 | 见类型定义 | 可为空 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "call_id",
    "state",
    "result",
    "error"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
