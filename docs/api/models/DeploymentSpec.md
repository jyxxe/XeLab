# DeploymentSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/DeploymentSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

部署期望配置；不直接填写容器地址

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `display_name` | string | 是 | minLength=`1`; maxLength=`80` | 部署名称 |
| `enabled` | boolean | 否 | default=`false` | 期望启用状态 |
| `replicas` | integer | 否 | minimum=`1`; maximum=`32`; default=`1` | 本机副本数；独占卷默认只能为 1 |
| `limits` | [ResourceLimits](ResourceLimits.md) | 是 | 见类型定义 | 实例获批上限；磁盘限额由配额存储实施 |
| `config` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `volume_ids` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`16` | 批准挂载的卷 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "installation_id",
    "display_name",
    "limits",
    "config",
    "volume_ids"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
