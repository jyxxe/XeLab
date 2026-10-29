# ModelAliasSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ModelAliasSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

内核批准的模型别名映射，不依赖插件私有配置推断

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^[a-zA-Z0-9][a-zA-Z0-9._/-]*$"` | 调用方使用的唯一模型别名 |
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `provider_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 固定提供者 |
| `context_tokens` | integer | 是 | minimum=`1`; maximum=`2097152` | 模型上下文 token 上限 |
| `maximum_output_tokens` | integer | 是 | minimum=`0`; maximum=`32768` | 生成上限；embedding 为 0 |
| `skill_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | skill_required 插件必需，须匹配既有 contract/provider 映射；本版仅ai能力目标，不接受micro或HTTP目标冒充AI适配 |
| `embedding_purpose` | string | 否 | default=`"semantic_similarity"`; enum=`["retrieval_query", "retrieval_document", "semantic_similarity"]` | 文本兼容入口的固定前缀用途 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "contract",
    "provider_deployment_id",
    "context_tokens",
    "maximum_output_tokens"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
