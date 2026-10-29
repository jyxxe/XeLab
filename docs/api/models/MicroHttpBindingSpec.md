# MicroHttpBindingSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroHttpBindingSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

管理者绑定固定endpoint及托管Secret；输入不含原始密钥/自选URL

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `endpoint_alias` | string | 是 | minLength=`1`; maxLength=`32`; pattern=`"^[a-z][a-z0-9-]{0,31}$"` | 不可变微清单中的endpoint别名 |
| `priority` | integer | 否 | minimum=`0`; maximum=`1000`; default=`10` | 越小越优先；同值按binding UUID稳定排序 |
| `enabled` | boolean | 否 | default=`false` |  |
| `secret_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 由secret.bind批准，只供核心代理注入 |
| `timeout_ms` | integer | 否 | minimum=`100`; maximum=`4000`; default=`4000` | 单次上游总时限，仍受根deadline剩余约束 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "endpoint_alias",
    "secret_id"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
