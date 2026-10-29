# QuotaSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/QuotaSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

账户树总额度；所有后代累计而非复制

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `maximum_depth` | integer | 是 | minimum=`1`; maximum=`8`; default=`4` | 含当前根在内的最大层级 |
| `maximum_descendants` | integer | 是 | minimum=`0`; maximum=`10000`; default=`20` | 后代总账户数 |
| `maximum_api_keys` | integer | 是 | minimum=`1`; maximum=`100000`; default=`50` | 子树内有效密钥数 |
| `maximum_concurrent_calls` | integer | 是 | minimum=`1`; maximum=`10000`; default=`16` | 子树并发调用 |
| `daily_calls` | integer | 是 | minimum=`1`; maximum=`9007199254740991`; default=`10000` | UTC 自然日子树调用量 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "maximum_depth",
    "maximum_descendants",
    "maximum_api_keys",
    "maximum_concurrent_calls",
    "daily_calls"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
