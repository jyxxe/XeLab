# SkillBatchItem

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillBatchItem.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

单项状态与结果引用；输入不在列表回显

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | minLength=`1`; maxLength=`80`; pattern=`"^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$"` | 调用方关联键；不是行ID或授权凭证 |
| `index` | integer | 是 | minimum=`0`; maximum=`999` | 提交顺序，从0开始 |
| `state` | string | 是 | enum=`["queued", "running", "succeeded", "failed", "canceled", "unknown"]` | unknown尚无可证实终态 |
| `call_id` | string / null | 是 | 见类型定义 |  |
| `attempts` | integer | 是 | minimum=`0`; maximum=`5` | 物理尝试数；不等同重复计费 |
| `started_at` | string / null | 是 | 见类型定义 |  |
| `completed_at` | string / null | 是 | 见类型定义 |  |
| `result_bytes` | integer / null | 是 | 见类型定义 |  |
| `result_digest` | string / null | 是 | 见类型定义 |  |
| `error` | [ErrorDetail](ErrorDetail.md) / null | 是 | 见类型定义 |  |
| `resolution_source` | string / null | 是 | 见类型定义 | 仅核实过unknown的项目非空；人工结果必须在UI标明 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "index",
    "state",
    "call_id",
    "attempts",
    "started_at",
    "completed_at",
    "result_bytes",
    "result_digest",
    "error",
    "resolution_source"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
