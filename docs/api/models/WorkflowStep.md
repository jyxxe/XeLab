# WorkflowStep

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/WorkflowStep.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

DAG 节点；contract 或 skill_id 二选一，Skill 节点锁定定义及提供者绑定

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | minLength=`1`; maxLength=`80`; pattern=`"^[a-z][a-z0-9_-]*$"` | 工作流内唯一标识 |
| `contract` | [ContractRef](ContractRef.md) | 否 | 见类型定义 | 不可变能力契约引用 |
| `depends_on` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`64` | 依赖 |
| `input_mapping` | [InputMapping](InputMapping.md) | 是 | 见类型定义 | 输出对象每个字段映射一个 MappingLeaf，禁止脚本 |
| `maximum_attempts` | integer | 否 | minimum=`1`; maximum=`5`; default=`1` | 总尝试数，含首次 |
| `timeout_ms` | integer | 是 | minimum=`100`; maximum=`86400000` | 单次期限 |
| `provider_deployment_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 状态相关步骤需固定提供者 |
| `skill_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 不可变的 Skill 版本 UUID；与 contract 二选一 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "depends_on",
    "input_mapping",
    "timeout_ms"
  ],
  "oneOf": [
    {
      "required": [
        "contract"
      ],
      "not": {
        "required": [
          "skill_id"
        ]
      }
    },
    {
      "required": [
        "skill_id"
      ],
      "not": {
        "anyOf": [
          {
            "required": [
              "contract"
            ]
          },
          {
            "required": [
              "provider_deployment_id"
            ]
          }
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
