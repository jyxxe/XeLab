# DependencySpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/DependencySpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

必要性、目标类型和绑定范围为独立维度

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 插件内唯一依赖别名 |
| `required` | boolean | 是 | 见类型定义 | 强依赖为 true |
| `target_kind` | string | 是 | enum=`["capability", "plugin"]` | 目标类型 |
| `target` | string | 是 | minLength=`1`; maxLength=`128` | 能力名称或插件 ID |
| `version_range` | string | 是 | minLength=`1`; maxLength=`128` | 支持 >=x.y.z <a.b.c 空格交集；不支持隐式 OR |
| `binding_scope` | string | 是 | enum=`["request", "deployment", "task"]` | 按调用、部署或任务固定提供者 |
| `failure_policy` | string | 是 | enum=`["wait", "fail", "fallback", "degrade"]` | 失效处理 |
| `features` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`256` | 必需特性 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "required",
    "target_kind",
    "target",
    "version_range",
    "binding_scope",
    "failure_policy",
    "features"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
