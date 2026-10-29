# MicroProxyResult

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroProxyResult.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

受控桥返回，不暴露响应头/鉴权值；非2xx不返回原始错误body

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `operation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `outcome` | string | 是 | enum=`["completed", "not_sent", "unknown"]` |  |
| `status_code` | integer / null | 是 | 见类型定义 |  |
| `body_json` | [JsonValue](JsonValue.md) / null | 是 | 见类型定义 |  |
| `error_code` | string / null | 是 | 见类型定义 |  |
| `retry_after_ms` | integer / null | 是 | 见类型定义 |  |
| `duration_ms` | integer | 是 | minimum=`0`; maximum=`4000` | 本次代理耗时 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "operation_id",
    "outcome",
    "status_code",
    "body_json",
    "error_code",
    "retry_after_ms",
    "duration_ms"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
