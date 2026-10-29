# ConsentDecision

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ConsentDecision.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

可信授权页提交；只接受绑定当前会话的 pending 请求

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `authorization_request_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `approved` | boolean | 是 | 见类型定义 | 是否批准预览中的固定权限范围 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "authorization_request_id",
    "approved"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
