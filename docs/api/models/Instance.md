# Instance

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/Instance.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

实际运行实例；控制器维护，插件不能自报 Ready 绕过准入

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `generation` | integer | 是 | minimum=`1`; maximum=`2147483647` | 所属期望配置代次 |
| `runtime_id` | string / null | 是 | 见类型定义 | 宿主运行时容器标识 |
| `phase` | string | 是 | enum=`["pending", "starting", "ready", "degraded", "draining", "stopped", "failed"]` | 实例状态 |
| `health` | string | 是 | enum=`["unknown", "alive", "unhealthy"]` | 探针状态 |
| `limits` | [ResourceLimits](ResourceLimits.md) | 是 | 见类型定义 | 实例获批上限；磁盘限额由配额存储实施 |
| `last_heartbeat_at` | string / null | 是 | 见类型定义 | 最近健康观测 |
| `restart_count` | integer | 是 | minimum=`0`; maximum=`10` | 本代次重启次数 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "id",
    "revision",
    "created_at",
    "updated_at",
    "deployment_id",
    "generation",
    "runtime_id",
    "phase",
    "health",
    "limits",
    "last_heartbeat_at",
    "restart_count"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
