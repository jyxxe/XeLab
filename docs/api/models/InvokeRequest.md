# InvokeRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/InvokeRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

通用业务调用；input 必须匹配注册契约；大文件传 Blob 引用

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `contract` | [ContractRef](ContractRef.md) | 是 | 见类型定义 | 不可变能力契约引用 |
| `input` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `provider_deployment_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 指定实现时仍校验权限及契约 |
| `timeout_ms` | integer | 否 | minimum=`100`; maximum=`300000`; default=`30000` | 整体期限；默认 30000 |
| `idempotency_key` | string | 否 | minLength=`1`; maxLength=`128` | 有副作用能力的稳定去重键，作用域见通信规范 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "contract",
    "input"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
