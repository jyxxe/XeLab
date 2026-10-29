# Quota

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Quota.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

账户树预算及当前使用

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `account_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `spec` | [QuotaSpec](QuotaSpec.md) | 是 | 见类型定义 | 账户树总额度；所有后代累计而非复制 |
| `descendants_used` | integer | 是 | minimum=`0`; maximum=`10000` | 当前后代数 |
| `api_keys_used` | integer | 是 | minimum=`0`; maximum=`100000` | 有效密钥数 |
| `concurrent_calls_used` | integer | 是 | minimum=`0`; maximum=`10000` | 当前并发 |
| `daily_calls_used` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | UTC 当日累计 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "account_id",
    "spec",
    "descendants_used",
    "api_keys_used",
    "concurrent_calls_used",
    "daily_calls_used"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
