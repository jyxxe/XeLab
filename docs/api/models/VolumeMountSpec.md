# VolumeMountSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/VolumeMountSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

声明卷需求而非宿主路径

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 插件内挂载别名 |
| `mount_path` | string | 是 | minLength=`1`; maxLength=`256`; pattern=`"^/"` | 容器绝对路径，禁止 /、/proc、/sys、/dev |
| `access` | string | 是 | enum=`["ro", "rw"]` | 读写模式 |
| `scope` | string | 是 | enum=`["instance", "deployment"]` | 卷归属范围 |
| `minimum_bytes` | integer | 是 | minimum=`1048576`; maximum=`9007199254740991` | 容量需求 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "mount_path",
    "access",
    "scope",
    "minimum_bytes"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
