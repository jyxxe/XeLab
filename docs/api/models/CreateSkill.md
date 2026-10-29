# CreateSkill

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateSkill.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

注册不可变Skill；普通来源须OCI签名覆盖，micro来源须核心候选与独立微制品匹配

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `definition` | [SkillDefinition](SkillDefinition.md) | 是 | 见类型定义 | 不可变的核心转发声明，不是可执行脚本或 Codex SKILL.md |
| `enabled` | boolean | 否 | default=`false` | 安装不等于启用 |
| `installation_id` | string | 是 | format=`"uuid"` | 普通Skill声明来源安装；micro Skill必须等于固定微版本的runtime_installation_id |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "definition",
    "installation_id"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
