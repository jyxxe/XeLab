# NodeConfig

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/NodeConfig.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

本机资源预算；profile 省略为 standard，compact 为整机至少 1 vCPU/512 MiB 的低并发档位；稳定性须另行宿主验收。

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `kernel_cpu_millis` | integer | 是 | minimum=`500`; maximum=`1048576`; default=`4000` | 内核预留；compact 至少 500mCPU，standard 至少 4000mCPU |
| `kernel_memory_bytes` | integer | 是 | minimum=`201326592`; maximum=`9007199254740991`; default=`17179869184` | 内核预留；compact 至少 192MiB，standard 至少 16GiB |
| `system_cpu_millis` | integer | 是 | minimum=`1`; maximum=`1048576` | 宿主/独立基础设施预留，必须显式配置 |
| `system_memory_bytes` | integer | 是 | minimum=`1048576`; maximum=`9007199254740991` | 宿主/基础设施预留，必须显式配置 |
| `shutdown_grace_ms` | integer | 是 | minimum=`1000`; maximum=`300000`; default=`30000` | 内核停止最大排空时间 |
| `profile` | string | 否 | default=`"standard"`; enum=`["standard", "compact"]` | 新节点初始化时固定的资源档位；已有节点不可在线或仅改配置切换，显式迁移尚未提供；旧配置保持 standard。 |

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
  ],
  "allOf": [
    {
      "if": {
        "properties": {
          "profile": {
            "const": "compact"
          }
        },
        "required": [
          "profile"
        ]
      },
      "then": {
        "properties": {
          "system_cpu_millis": {
            "minimum": 250
          },
          "system_memory_bytes": {
            "minimum": 268435456
          }
        }
      },
      "else": {
        "properties": {
          "kernel_cpu_millis": {
            "minimum": 4000
          },
          "kernel_memory_bytes": {
            "minimum": 17179869184
          }
        }
      }
    }
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
