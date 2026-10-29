# DependencyBinding

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/DependencyBinding.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

已解析提供者绑定

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `alias` | string | 是 | minLength=`1`; maxLength=`80` | 依赖别名 |
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 提供者部署 |
| `instance_id` | string / null | 是 | 见类型定义 | 按请求绑定时可以为空 |
| `binding_scope` | string | 是 | enum=`["request", "deployment", "task"]` | 范围 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "alias",
    "contract",
    "deployment_id",
    "instance_id",
    "binding_scope"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
