# CapabilitySkillTarget

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CapabilitySkillTarget.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

capability固定目标；其他种类字段拒绝，不改变既有wire结构

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kind` | JSON | 是 | 见类型定义 |  |
| `plugin_id` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$"` | 插件稳定 ID；不含版本 |
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "kind",
    "plugin_id",
    "contract"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
