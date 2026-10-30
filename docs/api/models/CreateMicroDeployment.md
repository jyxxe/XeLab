# CreateMicroDeployment

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/CreateMicroDeployment.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

绑定不可变微版本、业务limits与父MPR；资源/联网设置可在停用排空后通过settings接口手动修改；缺省资源250m/256MiB，联网关闭

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `micro_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `runtime_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `limits` | [MicroExecutionLimits](MicroExecutionLimits.md) | 是 | 见类型定义 | 不可变微版本/微部署的业务执行子限额；heap/stack不等于总RSS，CPU与总内存另见MicroResourceLimits |
| `resources` | [MicroResourceLimits](MicroResourceLimits.md) | 否 | default=`{"cpu_millis": 250, "memory_bytes": 268435456}` | 省略整对象由核心填入默认值；显式对象必须同时给出CPU和内存 |
| `network_enabled` | boolean | 否 | default=`false` | 显式联网意图；true只允许已声明controlled-http-v1，仍须调用权限/HttpBinding/父egress；false拒绝HTTP profile Invoke与ProxyHttp |

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
