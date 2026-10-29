# PluginApproval

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/PluginApproval.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

发布版本获批权限与外部网络访问；不是用户授权

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `rules` | array&lt;[PermissionRule](PermissionRule.md)&gt; | 是 | minItems=`0`; maxItems=`256` | 插件代理权限上限 |
| `egress` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`64` | 出口白名单 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "rules",
    "egress"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
