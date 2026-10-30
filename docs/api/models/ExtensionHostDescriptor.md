# ExtensionHostDescriptor

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ExtensionHostDescriptor.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

物理OCI根对ExtensionHostService的明确能力声明；缺省不支持，不因插件类型而自动推断

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `api_major` | JSON | 是 | 见类型定义 |  |
| `host_kinds` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`3` |  |
| `modes` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`2` |  |
| `maximum_chain_depth` | integer | 是 | minimum=`1`; maximum=`16`; default=`16` |  |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "api_major",
    "host_kinds",
    "modes",
    "maximum_chain_depth"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
