# DependencyPlan

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/DependencyPlan.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

可审核解析结果；执行前重新核对版本与资源

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 依据的部署版本 |
| `bindings` | array&lt;[DependencyBinding](DependencyBinding.md)&gt; | 是 | minItems=`0`; maxItems=`256` | 绑定结果 |
| `required_cpu_millis` | integer | 是 | minimum=`0`; maximum=`1048576` | 增量资源 |
| `required_memory_bytes` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | 增量内存 |
| `conflicts` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`64` | 冲突；非空不可执行 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "deployment_id",
    "revision",
    "bindings",
    "required_cpu_millis",
    "required_memory_bytes",
    "conflicts"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
