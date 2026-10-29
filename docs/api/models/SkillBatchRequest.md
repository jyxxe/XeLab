# SkillBatchRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillBatchRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

持久化异步批次；完整JSON最大4MiB，所有项目使用同一固定Skill版本

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `items` | array&lt;[SkillBatchInputItem](SkillBatchInputItem.md)&gt; | 是 | minItems=`1`; maxItems=`1000` | 提交前全部按目标输入Schema校验；同批id重复拒绝 |
| `item_timeout_ms` | integer | 否 | minimum=`100`; maximum=`300000`; default=`120000` | 显式值不超过Skill上限；省略取min(120000,Skill.maximum_timeout_ms) |
| `timeout_ms` | integer | 否 | minimum=`1000`; maximum=`604800000`; default=`86400000` | 整批期限，不能延长源授权有效期 |
| `max_parallel` | integer | 否 | minimum=`1`; maximum=`16`; default=`1` | 整批在途项目上限，实际还受用户/实例/节点额度限制 |
| `failure_policy` | string | 否 | default=`"continue"`; enum=`["continue", "stop"]` | continue继续独立项目；stop停止领取新项目，不回滚已提交项 |
| `run_as` | string | 否 | default=`"user"`; enum=`["user", "service"]` | 默认 user；service 需要独立批准 |
| `service_account_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | run_as=service 时必填 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "items"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "run_as": {
            "const": "service"
          }
        },
        "required": [
          "run_as"
        ]
      },
      "then": {
        "required": [
          "service_account_id"
        ]
      },
      "else": {
        "not": {
          "required": [
            "service_account_id"
          ]
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
