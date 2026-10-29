# SkillStreamEvent

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillStreamEvent.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

Skill SSE 信封；event 的序列和终态规则不变

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `skill_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | UUID |
| `skill_version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `definition_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `event` | [StreamEvent](StreamEvent.md) | 是 | 见类型定义 | SSE data 对象；terminal 只出现一次 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "skill_id",
    "skill_version",
    "definition_digest",
    "event"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
