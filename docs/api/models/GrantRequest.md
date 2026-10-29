# GrantRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/GrantRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

为路径账户添加授权；不改变父级上限

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `source` | string | 是 | enum=`["role", "group"]` | 来源类型 |
| `role_id` | string | 否 | enum=`["Owner", "Operator", "Member", "Auditor", "Guest"]` | source=role 时必填 |
| `group_version` | [GroupVersionRef](GroupVersionRef.md) | 否 | 见类型定义 | 固定版本权限组引用 |
| `scope` | [ResourceSelector](ResourceSelector.md) | 是 | 见类型定义 | 资源选择器；all 仅表示指定类型内所有资源 |
| `expires_at` | string | 否 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 可选有效期 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "source",
    "scope"
  ],
  "oneOf": [
    {
      "properties": {
        "source": {
          "const": "role"
        }
      },
      "required": [
        "role_id"
      ],
      "not": {
        "required": [
          "group_version"
        ]
      }
    },
    {
      "properties": {
        "source": {
          "const": "group"
        }
      },
      "required": [
        "group_version"
      ],
      "not": {
        "required": [
          "role_id"
        ]
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
