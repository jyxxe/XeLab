# HealthCheckSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/HealthCheckSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

插件启动、存活和就绪检查

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `startup_timeout_ms` | integer | 是 | minimum=`1000`; maximum=`600000`; default=`60000` | 启动检查总期限 |
| `interval_ms` | integer | 是 | minimum=`1000`; maximum=`60000`; default=`5000` | 存活检查间隔 |
| `timeout_ms` | integer | 是 | minimum=`100`; maximum=`10000`; default=`2000` | 单次检查期限 |
| `failure_threshold` | integer | 是 | minimum=`1`; maximum=`10`; default=`3` | 连续失败次数 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "startup_timeout_ms",
    "interval_ms",
    "timeout_ms",
    "failure_threshold"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
