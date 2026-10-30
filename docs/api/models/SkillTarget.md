# SkillTarget

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/SkillTarget.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

四种互斥固定目标；extension通过核心验证全宿主链，不接受即时源码/任意URL或运行时代次替换

## 字段

类型：[CapabilitySkillTarget](CapabilitySkillTarget.md) / [HttpSkillTarget](HttpSkillTarget.md) / [MicroSkillTarget](MicroSkillTarget.md) / [ExtensionSkillTarget](ExtensionSkillTarget.md)。四种互斥固定目标；extension通过核心验证全宿主链，不接受即时源码/任意URL或运行时代次替换

## 对象及跨字段约束

```json
{
  "oneOf": [
    {
      "$ref": "#/components/schemas/CapabilitySkillTarget"
    },
    {
      "$ref": "#/components/schemas/HttpSkillTarget"
    },
    {
      "$ref": "#/components/schemas/MicroSkillTarget"
    },
    {
      "$ref": "#/components/schemas/ExtensionSkillTarget"
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
