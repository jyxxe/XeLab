# UiContribution

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UiContribution.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

按当前用户过滤后的已批准UI；地址由核心受控origin映射产生

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `plugin_id` | string | 是 | minLength=`1`; maxLength=`128`; pattern=`"^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$"` | 插件稳定 ID；不含版本 |
| `manifest_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `contribution_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `spec` | [UiContributionSpec](UiContributionSpec.md) | 是 | 见类型定义 | 可选独立iframe窗口或桌面组件；两种均不得将插件JS注入桌面壳 |
| `frame_url` | string / null | 是 | 见类型定义 | 未配置隔离源时null；ready必须为受控HTTPS地址 |
| `frame_origin` | string / null | 是 | 见类型定义 | 未配置隔离源时null；ready必须为受控HTTPS地址 |
| `availability` | string | 是 | enum=`["ready", "unavailable"]` | 无隔离origin/实例未就绪/绑定缺失时显示原因 |
| `reason_codes` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`32` | 稳定错误原因，不能泄露内部端点 |
| `bindings` | array&lt;[UiResolvedSkillBinding](UiResolvedSkillBinding.md)&gt; | 是 | minItems=`0`; maxItems=`32` | 当前获准的桥接绑定；未获准绑定省略 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "deployment_id",
    "installation_id",
    "plugin_id",
    "manifest_digest",
    "contribution_digest",
    "spec",
    "frame_url",
    "frame_origin",
    "availability",
    "reason_codes",
    "bindings"
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "availability": {
            "const": "ready"
          }
        },
        "required": [
          "availability"
        ]
      },
      "then": {
        "properties": {
          "frame_url": {
            "type": "string"
          },
          "frame_origin": {
            "type": "string"
          }
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
