# ModelAlias

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ModelAlias.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

对调用者可见的模型别名

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | minLength=`1`; maxLength=`128` | 模型别名 |
| `object` | string | 是 | enum=`["model"]` | 对象类型 |
| `owned_by` | string | 是 | minLength=`1`; maxLength=`128` | 发布者显示名称 |
| `created` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | Unix 秒 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "object",
    "owned_by",
    "created"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
