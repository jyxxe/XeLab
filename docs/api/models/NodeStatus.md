# NodeStatus

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/NodeStatus.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

单节点预算与实际用量

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `revision` | integer | 是 | minimum=`1`; maximum=`2147483647` | 乐观锁版本，每次持久化变更加一 |
| `created_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 创建时间，UTC |
| `updated_at` | string | 是 | format=`"date-time"`; minLength=`1`; maxLength=`40` | 最近变更时间，UTC |
| `config` | [NodeConfig](NodeConfig.md) | 是 | 见类型定义 | 本机资源预算；内核最低预留不能调低 |
| `host_cpu_millis` | integer | 是 | minimum=`1`; maximum=`1048576` | 发现的宿主 CPU 容量 |
| `host_memory_bytes` | integer | 是 | minimum=`1`; maximum=`9007199254740991` | 宿主物理内存 |
| `plugin_cpu_pool_millis` | integer | 是 | minimum=`0`; maximum=`1048576` | 扣除内核和系统预留后的 CPU 容量 |
| `plugin_memory_pool_bytes` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | 扣除预留后的内存容量 |
| `reserved_cpu_millis` | integer | 是 | minimum=`0`; maximum=`1048576` | 插件已准入总 CPU |
| `reserved_memory_bytes` | integer | 是 | minimum=`0`; maximum=`9007199254740991` | 插件已准入总内存 |
| `admission_enabled` | boolean | 是 | 见类型定义 | 对账完成且预算有效才可接受新实例 |

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
    "config",
    "host_cpu_millis",
    "host_memory_bytes",
    "plugin_cpu_pool_millis",
    "plugin_memory_pool_bytes",
    "reserved_cpu_millis",
    "reserved_memory_bytes",
    "admission_enabled"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
