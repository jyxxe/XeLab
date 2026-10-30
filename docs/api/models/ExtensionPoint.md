# ExtensionPoint

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ExtensionPoint.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

版本化宿主扩展点；data有声明式JSON适配器，skill仅调用固定获准处理器；点内Schema与契约包摘要必须匹配

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `point_id` | string | 是 | minLength=`1`; maxLength=`80`; pattern=`"^[a-z][a-z0-9.-]+$"` | 宿主不可变清单内唯一扩展点ID |
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `mode` | string | 是 | enum=`["data", "skill"]` |  |
| `input_schema` | [MicroBusinessSchema](MicroBusinessSchema.md) | 是 | 见类型定义 | 受限JSON Schema2020-12；每份<=64KiB、<=256节点/深度16，无$ref/正则/自定义format或资源能力标记 |
| `output_schema` | [MicroBusinessSchema](MicroBusinessSchema.md) | 是 | 见类型定义 | 受限JSON Schema2020-12；每份<=64KiB、<=256节点/深度16，无$ref/正则/自定义format或资源能力标记 |
| `contribution_schema` | [MicroBusinessSchema](MicroBusinessSchema.md) | 是 | 见类型定义 | 受限JSON Schema2020-12；每份<=64KiB、<=256节点/深度16，无$ref/正则/自定义format或资源能力标记 |
| `config_schema` | [MicroBusinessSchema](MicroBusinessSchema.md) | 是 | 见类型定义 | 受限JSON Schema2020-12；每份<=64KiB、<=256节点/深度16，无$ref/正则/自定义format或资源能力标记 |
| `side_effect` | string | 是 | enum=`["none", "idempotent", "non_repeatable"]` | 宿主点及原业务契约允许的副作用分类；处理器不得比宿主点/根profile更宽，idempotent须有可靠结果账本 |
| `maximum_timeout_ms` | integer | 是 | minimum=`100`; maximum=`30000`; default=`2000` | 包含宿主、处理器、排队及完整扩展链的总期限上限 |
| `maximum_input_bytes` | integer | 是 | minimum=`1`; maximum=`1048576`; default=`1048576` |  |
| `maximum_output_bytes` | integer | 是 | minimum=`1`; maximum=`1048576`; default=`1048576` |  |
| `maximum_contributions` | integer | 是 | minimum=`1`; maximum=`128`; default=`16` | 同一点最大活跃贡献数量，按priority/UUID稳定排序 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "point_id",
    "contract",
    "mode",
    "input_schema",
    "output_schema",
    "contribution_schema",
    "config_schema",
    "side_effect",
    "maximum_timeout_ms",
    "maximum_input_bytes",
    "maximum_output_bytes",
    "maximum_contributions"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
