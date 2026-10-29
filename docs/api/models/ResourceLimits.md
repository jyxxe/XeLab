# ResourceLimits

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ResourceLimits.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

实例获批上限；磁盘限额由配额存储实施

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `cpu_millis` | integer | 是 | minimum=`10`; maximum=`1048576` | 1000=1 vCPU |
| `memory_bytes` | integer | 是 | minimum=`16777216`; maximum=`9007199254740991` | 字节；最低 16 MiB |
| `ephemeral_bytes` | integer | 是 | minimum=`1048576`; maximum=`9007199254740991` | 临时存储字节 |
| `pids` | integer | 是 | minimum=`16`; maximum=`65536` | 进程/线程计数上限 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "cpu_millis",
    "memory_bytes",
    "ephemeral_bytes",
    "pids"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
