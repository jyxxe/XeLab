# Client

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Client.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

WebUI 客户端

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `client_id` | string | 是 | minLength=`1`; maxLength=`128` | 公开标识，不是工作负载身份证明 |
| `spec` | [ClientSpec](ClientSpec.md) | 是 | 见类型定义 | 浏览器公共客户端注册信息；不分配前端 client_secret |
| `status` | string | 是 | enum=`["active", "disabled"]` | 状态 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "client_id",
    "spec",
    "status"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
