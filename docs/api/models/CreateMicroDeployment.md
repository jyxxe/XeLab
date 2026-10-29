# CreateMicroDeployment

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateMicroDeployment.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

独立绑定版本与MPR父部署；初始关闭，绑定/预算修改需新建

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `micro_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `runtime_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `limits` | [MicroExecutionLimits](MicroExecutionLimits.md) | 是 | 见类型定义 | 获批执行预算，必须同时不超过版本声明和MPR上限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "micro_version_id",
    "runtime_deployment_id",
    "limits"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
