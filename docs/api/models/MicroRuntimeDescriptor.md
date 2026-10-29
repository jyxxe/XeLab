# MicroRuntimeDescriptor

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroRuntimeDescriptor.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

普通OCI插件可提供的受审微执行适配协议声明，不是业务能力

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `api_major` | JSON | 是 | 见类型定义 |  |
| `engines` | array&lt;JSON&gt; | 是 | minItems=`1`; maxItems=`1` |  |
| `protocols` | array&lt;JSON&gt; | 是 | minItems=`1`; maxItems=`1` |  |
| `execution_profiles` | array&lt;string&gt; | 否 | minItems=`1`; maxItems=`2`; default=`["pure-json-v1"]` |  |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "api_major",
    "engines",
    "protocols"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
