# NodeConfig

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/NodeConfig.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

本机资源预算；内核最低预留不能调低

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kernel_cpu_millis` | integer | 是 | minimum=`4000`; maximum=`1048576`; default=`4000` | 内核预留，至少 4 vCPU |
| `kernel_memory_bytes` | integer | 是 | minimum=`17179869184`; maximum=`9007199254740991`; default=`17179869184` | 内核预留，至少 16 GiB |
| `system_cpu_millis` | integer | 是 | minimum=`1`; maximum=`1048576` | 宿主/独立基础设施预留，必须显式配置 |
| `system_memory_bytes` | integer | 是 | minimum=`1048576`; maximum=`9007199254740991` | 宿主/基础设施预留，必须显式配置 |
| `shutdown_grace_ms` | integer | 是 | minimum=`1000`; maximum=`300000`; default=`30000` | 内核停止最大排空时间 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "kernel_cpu_millis",
    "kernel_memory_bytes",
    "system_cpu_millis",
    "system_memory_bytes",
    "shutdown_grace_ms"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
