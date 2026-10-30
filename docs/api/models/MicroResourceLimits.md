# MicroResourceLimits

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/MicroResourceLimits.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

同一微部署全部活动worker合计的cgroup CPU/内存硬限制，不是JS heap或每Call复制预算；由处理器在创建时补齐整对象缺省值

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `cpu_millis` | integer | 是 | minimum=`10`; maximum=`8000`; default=`250` | 1000m=1vCPU；cpu.max周期100000us、quota=cpu_millis×100us，聚合同部署所有worker |
| `memory_bytes` | integer | 是 | minimum=`67108864`; maximum=`17179869184`; default=`268435456` | 总memory.max，默认256MiB；含worker引擎/堆/栈/代码/I/O/内核计费，不含父共享制品缓存与监督器 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "required": [
    "cpu_millis",
    "memory_bytes"
  ],
  "additionalProperties": false
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
