# AuthorizationDecision

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/AuthorizationDecision.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

决策只供界面提示；不能替代执行时重新鉴权

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `allowed` | boolean | 是 | 见类型定义 | 是否允许 |
| `code` | string | 是 | enum=`["ALLOW", "NO_GRANT", "PARENT_DISABLED", "CREDENTIAL_REVOKED", "SCOPE_MISMATCH", "POLICY_DENIED"]` | 决策码 |
| `authz_revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 决策使用的版本 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "allowed",
    "code",
    "authz_revision"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
