# UiContributionKey

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiContributionKey.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

用户布局只引用具体部署与贡献，不跨新安装静默恢复

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `contribution_id` | string | 是 | minLength=`1`; maxLength=`64`; pattern=`"^[a-z][a-z0-9_-]{0,63}$"` | 插件内稳定标识 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "deployment_id",
    "contribution_id"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
