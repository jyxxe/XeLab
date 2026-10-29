# MicroExecutionLimits

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroExecutionLimits.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

获批执行预算，必须同时不超过版本声明和MPR上限

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `timeout_ms` | integer | 是 | minimum=`100`; maximum=`30000`; default=`2000` | 含Prepare/排队/编译/执行的总期限 |
| `heap_bytes` | integer | 是 | minimum=`1048576`; maximum=`67108864`; default=`16777216` | 每VM可计量堆上限；不是独立cgroup RSS |
| `stack_bytes` | integer | 是 | minimum=`65536`; maximum=`1048576`; default=`1048576` | 每VM堆栈硬上限 |
| `maximum_input_bytes` | integer | 是 | minimum=`1`; maximum=`1048576`; default=`1048576` | UTF8 JSON输入字节上限 |
| `maximum_output_bytes` | integer | 是 | minimum=`1`; maximum=`1048576`; default=`1048576` | 完整UTF8 JSON输出字节上限 |
| `maximum_stderr_bytes` | integer | 是 | minimum=`0`; maximum=`65536`; default=`65536` | 独立收集stderr上限；超出截断并标记 |
| `max_active_requests` | integer | 是 | minimum=`1`; maximum=`4`; default=`1` | 同微部署并发，再与父实例活动池求交 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "timeout_ms",
    "heap_bytes",
    "stack_bytes",
    "maximum_input_bytes",
    "maximum_output_bytes",
    "maximum_stderr_bytes",
    "max_active_requests"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
