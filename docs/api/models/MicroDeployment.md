# MicroDeployment

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroDeployment.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

微插件运行绑定；不创建OCI容器，候选Skill在此生成

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `owner_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_plugin_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `micro_version_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `runtime_deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `runtime_installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `package_digest` | string | 是 | minLength=`1`; maxLength=`71`; pattern=`"^sha256:[a-f0-9]{64}$"` | 制品或内容 SHA-256 摘要 |
| `limits` | [MicroExecutionLimits](MicroExecutionLimits.md) | 是 | 见类型定义 | 不可变微版本/微部署的业务执行子限额；heap/stack不等于总RSS，CPU与总内存另见MicroResourceLimits |
| `enabled` | boolean | 是 | 见类型定义 |  |
| `generation` | integer | 是 | minimum=`1`; maximum=`2147483647` | 期望启停或资源/联网设置发生实质变更时递增；旧Prepare句柄和迟到执行不得覆盖新代次 |
| `phase` | string | 是 | enum=`["disabled", "preparing", "ready", "draining", "waiting_runtime", "failed", "archived"]` | 观察状态；停用先进入draining，队列/Prepare/worker/句柄与组回收完成才disabled |
| `reason_codes` | array&lt;string&gt; | 是 | minItems=`0`; maxItems=`32` | 当前阻塞原因 |
| `candidate_skill` | [SkillDefinition](SkillDefinition.md) | 是 | 见类型定义 | 不可变的核心转发声明，不是可执行脚本或 Codex SKILL.md |
| `resources` | [MicroResourceLimits](MicroResourceLimits.md) | 是 | 见类型定义 | 同一微部署全部活动worker合计的cgroup CPU/内存硬限制，不是JS heap或每Call复制预算；由处理器在创建时补齐整对象缺省值 |
| `network_enabled` | boolean | 是 | default=`false` | 显式联网意图；true只允许已声明controlled-http-v1，仍须调用权限/HttpBinding/父egress；false拒绝HTTP profile Invoke与ProxyHttp |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "owner_id",
    "micro_plugin_id",
    "micro_version_id",
    "runtime_deployment_id",
    "runtime_installation_id",
    "package_digest",
    "limits",
    "enabled",
    "generation",
    "phase",
    "reason_codes",
    "candidate_skill",
    "resources",
    "network_enabled"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
