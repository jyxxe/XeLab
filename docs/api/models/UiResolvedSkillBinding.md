# UiResolvedSkillBinding

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiResolvedSkillBinding.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

仅返回当前调用者获准的固定Skill绑定

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `alias` | string | 是 | minLength=`1`; maxLength=`64`; pattern=`"^[a-z][a-z0-9_-]{0,63}$"` | 贡献内唯一别名 |
| `skill_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `definition_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `modes` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`2` | 允许的宿主桥方法；未声明默认全部禁止 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "alias",
    "skill_id",
    "definition_digest",
    "modes"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
