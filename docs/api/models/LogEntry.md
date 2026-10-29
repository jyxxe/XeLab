# LogEntry

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/LogEntry.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

脱敏结构化日志；需要单独日志读取权限

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 日志时间 |
| `level` | string | 是 | enum=`["debug", "info", "warn", "error"]` | 级别 |
| `instance_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `request_id` | string / null | 是 | 见类型定义 | 资源唯一标识 |
| `message` | string | 是 | minLength=`1`; maxLength=`8192` | 脱敏日志文本 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "at",
    "level",
    "instance_id",
    "request_id",
    "message"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
