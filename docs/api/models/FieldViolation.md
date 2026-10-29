# FieldViolation

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/FieldViolation.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

字段级错误

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `path` | string | 是 | minLength=`1`; maxLength=`512` | JSON Pointer 或参数名 |
| `reason` | string | 是 | minLength=`1`; maxLength=`1024` | 可读原因 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "path",
    "reason"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
