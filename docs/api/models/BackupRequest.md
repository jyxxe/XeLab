# BackupRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/BackupRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

创建一致性备份；数据卷冻结由批准的插件迁移/排空流程执行

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `volume_ids` | array&lt;string&gt; | 是 | minItems=`1`; maxItems=`16` | 同一一致性组的卷 |
| `deployment_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `destination` | string | 是 | enum=`["off_host"]` | 宿主外备份存储；连接配置由宿主管理 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "volume_ids",
    "deployment_id",
    "destination"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
