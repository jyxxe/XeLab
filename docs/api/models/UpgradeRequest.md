# UpgradeRequest

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/UpgradeRequest.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

升级或回滚到已安装的明确目标版本

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `installation_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `strategy` | string | 是 | enum=`["recreate", "parallel_compatible"]` | 必须与卷和数据兼容能力一致 |
| `backup_id` | string | 否 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 涉及迁移时需要可用备份 |
| `allow_downtime` | boolean | 是 | default=`false` | recreate 必须明确允许维护中断 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "installation_id",
    "strategy",
    "allow_downtime"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
