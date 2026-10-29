# CallResult

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CallResult.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

非流式结果；micro调用必须附全部微绑定/制品标识，contract使用候选Skill的name/version/definition_digest

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `call_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `provider_instance_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `output` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `completed_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 完成时间 |
| `skill_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 经 Skill 调用时必有；直接能力调用省略 |
| `skill_definition_digest` | string | 否 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `micro_version_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_deployment_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `package_digest` | string | 否 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `micro_plugin_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "call_id",
    "contract",
    "provider_instance_id",
    "output",
    "completed_at"
  ],
  "dependentRequired": {
    "micro_plugin_id": [
      "micro_version_id",
      "micro_deployment_id",
      "package_digest",
      "skill_id",
      "skill_definition_digest"
    ],
    "micro_version_id": [
      "micro_plugin_id",
      "micro_deployment_id",
      "package_digest",
      "skill_id",
      "skill_definition_digest"
    ],
    "micro_deployment_id": [
      "micro_plugin_id",
      "micro_version_id",
      "package_digest",
      "skill_id",
      "skill_definition_digest"
    ],
    "package_digest": [
      "micro_plugin_id",
      "micro_version_id",
      "micro_deployment_id",
      "skill_id",
      "skill_definition_digest"
    ]
  }
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
