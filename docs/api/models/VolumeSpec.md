# VolumeSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/VolumeSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

平台管理卷；禁止指定任意宿主路径

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 显示名称 |
| `capacity_bytes` | integer | 是 | minimum=`1048576`; maximum=`9007199254740991` | 硬配额字节数 |
| `access` | string | 是 | enum=`["single_writer", "shared_readonly"]` | 并发写入能力 |
| `scope` | string | 是 | enum=`["instance", "deployment"]` | 绑定范围 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "capacity_bytes",
    "access",
    "scope"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
