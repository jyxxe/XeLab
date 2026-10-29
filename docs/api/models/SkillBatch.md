# SkillBatch

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillBatch.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

核心持久化业务批次；不是管理Operation，也不要求创建Workflow

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `owner_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `subject_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `delegation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `skill_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `skill_version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `definition_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `state` | string | 是 | enum=`["queued", "running", "waiting_dependency", "paused_authorization", "cancel_requested", "succeeded", "partially_succeeded", "failed", "canceled", "unknown"]` | 批次状态；unknown禁止盲重放 |
| `counts` | [SkillBatchCounts](SkillBatchCounts.md) | 是 | 见类型定义 | 同一读快照的计数；各状态之和必须等于total |
| `max_parallel` | integer | 是 | minimum=`1`; maximum=`16` | 实际配置的批次并行上限 |
| `failure_policy` | string | 是 | enum=`["continue", "stop"]` | 逐项失败策略 |
| `run_as` | string | 是 | enum=`["user", "service"]` | 实际执行主体模式 |
| `service_account_id` | string / null | 是 | 见类型定义 |  |
| `deadline_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `completed_at` | string / null | 是 | 见类型定义 |  |
| `results_expires_at` | string / null | 是 | 见类型定义 |  |
| `error` | [ErrorDetail](ErrorDetail.md) / null | 是 | 见类型定义 |  |

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
    "owner_id",
    "subject_id",
    "delegation_id",
    "skill_id",
    "skill_version",
    "definition_digest",
    "installation_id",
    "state",
    "counts",
    "max_parallel",
    "failure_policy",
    "run_as",
    "service_account_id",
    "deadline_at",
    "completed_at",
    "results_expires_at",
    "error"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
