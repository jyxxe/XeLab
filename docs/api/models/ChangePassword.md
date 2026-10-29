# ChangePassword

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ChangePassword.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

用户本人修改密码；服务账户不支持

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `current_password` | string | 是 | minLength=`1`; maxLength=`128`; writeOnly=`true` | 当前密码 |
| `new_password` | string | 是 | minLength=`12`; maxLength=`128`; writeOnly=`true` | 新密码，12–128 字符 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "current_password",
    "new_password"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
