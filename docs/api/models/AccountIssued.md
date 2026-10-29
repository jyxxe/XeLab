# AccountIssued

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/AccountIssued.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

创建结果；激活凭证只在首次成功响应展示

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `account` | [Account](Account.md) | 是 | 见类型定义 | 用户或服务账户；API Key 不属于账户 |
| `activation_token` | string / null | 是 | 见类型定义 | 仅用户账户返回；有效期 24h；重放响应不含明文 |
| `activation_expires_at` | string / null | 是 | 见类型定义 | 激活凭证到期时间 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "account",
    "activation_token",
    "activation_expires_at"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
