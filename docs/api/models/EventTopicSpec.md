# EventTopicSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/EventTopicSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

插件声明的事件主题契约

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `topic` | string | 是 | minLength=`1`; maxLength=`160` | 必须位于 plugin_id 命名空间内 |
| `version` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)\\.(0&#124;[1-9][0-9]*)(?:-((?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0&#124;[1-9][0-9]*&#124;[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$"` | SemVer 2.0.0；版本约束语法见插件版本文档 |
| `payload_schema` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `schema_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "topic",
    "version",
    "payload_schema",
    "schema_digest"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
