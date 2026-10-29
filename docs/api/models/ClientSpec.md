# ClientSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ClientSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

浏览器公共客户端注册信息；不分配前端 client_secret

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 客户端显示名称 |
| `plugin_id` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$"` | 插件稳定 ID；不含版本 |
| `redirect_uris` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`8` | 回调白名单 |
| `allowed_origins` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`8` | 跨源白名单 |
| `maximum_rules` | array&lt;[PermissionRule](PermissionRule.md)&gt; | 是 | minItems=`1`; maxItems=`256` | 可请求委托上限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "plugin_id",
    "redirect_uris",
    "allowed_origins",
    "maximum_rules"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
