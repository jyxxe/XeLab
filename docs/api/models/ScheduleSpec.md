# ScheduleSpec

[模型目录](README.md) · [完整机器 Schema](../../contracts/schemas/ScheduleSpec.schema.json)

> 状态：规范性设计；由 OpenAPI 生成。

使用 IANA 时区的五字段 cron；明确 DST 和错过触发规则

## 字段

| 字段 | 类型 | 必填 | 约束 / 默认值 | 含义 |
|---|---|---|---|---|
| `name` | string | 是 | minLength=`1`; maxLength=`80` | 名称 |
| `workflow_id` | string | 是 | format=`"uuid"`; minLength=`1`; maxLength=`36` | 资源唯一标识 |
| `cron` | string | 是 | minLength=`1`; maxLength=`128` | 分 时 日 月 周；支持 *、范围、列表、步长 |
| `timezone` | string | 否 | minLength=`1`; maxLength=`80`; default=`"Asia/Shanghai"` | IANA 时区 |
| `input` | [JsonValue](JsonValue.md) | 是 | 见类型定义 | 业务契约定义的 JSON 值；业务输入输出必须另按对应能力的 JSON Schema 校验。控制面字段不能用此类型逃避定义。 |
| `enabled` | boolean | 否 | default=`true` | 是否触发 |
| `misfire` | string | 否 | default=`"skip"`; enum=`["skip", "run_once"]` | 停机错过时刻的处理 |
| `overlap` | string | 否 | default=`"skip"`; enum=`["skip", "queue_one"]` | 同一计划同时执行策略 |

## 对象及跨字段约束

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "name",
    "workflow_id",
    "cron",
    "input"
  ]
}
```

完整字段结构和嵌套引用见本页顶部机器 Schema。`additionalProperties=false` 的对象拒绝未知字段；可选与 nullable 是独立概念。跨字段条件、oneOf / allOf 均须执行。Schema 中的引用位于 [OpenAPI](../../contracts/openapi.json) 的 components.schemas。
