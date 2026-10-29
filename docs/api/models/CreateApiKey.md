# CreateApiKey

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateApiKey.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

创建密钥；全量模式也保留实际创建凭证来源

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 用于识别用途 |
| `inheritance` | [InheritanceSpec](InheritanceSpec.md) | 是 | 见类型定义 | 用户选择的继承方式；快照上限由内核展开并保存 |
| `conditions` | [CredentialConditions](CredentialConditions.md) | 是 | 见类型定义 | 密钥附加条件，创建者源凭证条件也持续适用 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "inheritance",
    "conditions"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
