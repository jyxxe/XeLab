# MicroDeploymentSettings

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroDeploymentSettings.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

完整替换微部署资源与联网设置；仅已停用且排空时提交，不修改业务limits、版本、父绑定或候选Skill

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `resources` | [MicroResourceLimits](MicroResourceLimits.md) | 是 | 见类型定义 | 同一微部署全部活动worker合计的cgroup CPU/内存硬限制，不是JS heap或每Call复制预算；由处理器在创建时补齐整对象缺省值 |
| `network_enabled` | boolean | 是 | default=`false` | 显式联网意图；true只允许已声明controlled-http-v1，仍须调用权限/HttpBinding/父egress；false拒绝HTTP profile Invoke与ProxyHttp |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "resources",
    "network_enabled"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
