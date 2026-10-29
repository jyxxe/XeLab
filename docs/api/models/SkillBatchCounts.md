# SkillBatchCounts

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillBatchCounts.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

同一读快照的计数；各状态之和必须等于total

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `total` | integer | 是 | minimum=`1`; maximum=`1000` | 提交项总数，创建后固定 |
| `queued` | integer | 是 | minimum=`0`; maximum=`1000` | 该状态项数 |
| `running` | integer | 是 | minimum=`0`; maximum=`1000` | 该状态项数 |
| `succeeded` | integer | 是 | minimum=`0`; maximum=`1000` | 该状态项数 |
| `failed` | integer | 是 | minimum=`0`; maximum=`1000` | 该状态项数 |
| `canceled` | integer | 是 | minimum=`0`; maximum=`1000` | 该状态项数 |
| `unknown` | integer | 是 | minimum=`0`; maximum=`1000` | 该状态项数 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "total",
    "queued",
    "running",
    "succeeded",
    "failed",
    "canceled",
    "unknown"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
