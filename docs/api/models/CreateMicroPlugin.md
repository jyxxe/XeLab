# CreateMicroPlugin

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateMicroPlugin.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

owner取当前认证主体；name在owner命名空间唯一

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`64`; pattern=`"^[a-z][a-z0-9-]{0,63}$"` | 稳定名称，创建后不变 |
| `display_name` | string | 是 | minLength=`1`; maxLength=`128` | 普通文本显示名 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "name",
    "display_name"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
