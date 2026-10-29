# Skill

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Skill.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

Skill 版本与当前可用性

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `definition` | [SkillDefinition](SkillDefinition.md) | 是 | 见类型定义 | 不可变的核心转发声明，不是可执行脚本或 Codex SKILL.md |
| `definition_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `enabled` | boolean | 是 | 见类型定义 | 期望启用状态 |
| `availability` | string | 是 | enum=`["ready", "unavailable", "disabled"]` | 健康/依赖/审批过滤结果 |
| `reason_codes` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`32` | 不可用原因 |
| `installation_id` | string | 是 | format=`"uuid"` | 普通Skill声明来源安装；micro Skill必须等于固定微版本的runtime_installation_id |

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
    "definition",
    "definition_digest",
    "enabled",
    "availability",
    "reason_codes",
    "installation_id"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
