# HealthStatus

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/HealthStatus.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

无敏感详情的探针返回

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `status` | string | 是 | enum=`["ok", "not_ready"]` | 存活/就绪状态 |
| `spec_version` | string | 是 | minLength=`1`; maxLength=`32` | 平台规格版本 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "status",
    "spec_version"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
