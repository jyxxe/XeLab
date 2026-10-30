# 日期时间转换接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。

## `datetime.convert` / `convert`

[输入 Schema](contracts/convert/input.schema.json) · [输出 Schema](contracts/convert/output.schema.json) · [流式声明](contracts/convert/stream.schema.json) · [行为契约](contracts/convert/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `value` | 必填日期时间字符串；不得用宿主宽松解析 |
| `input_format` | 可选，默认 `RFC3339`；只接受制品明确列出的严格格式 |
| `source_timezone` | 可选 `UTC`、固定偏移或受支持 IANA 标识；仅用于无偏移的受支持本地输入格式 |
| `target_timezone` | 可选，默认 `UTC`；同上，IANA 必须满足 tzdb 门禁 |
| `output_format` | 可选，默认 `RFC3339`；只接受制品明确列出的格式 |

输出 `{value: string, utc_value: string, timezone: string, offset_seconds: integer}`：`value` 为目标格式字符串；`utc_value` 为同一瞬间的 UTC RFC3339 表示；`timezone` 回显规范化目标时区；`offset_seconds` 是该瞬间目标时区相对 UTC 的秒偏移。

基础草案只承诺带 `Z` 或数值偏移的 RFC3339 输入，以及 RFC3339 输出。固定偏移为 `±HH:MM`，范围 `-14:00`..`+14:00`；绝对值 14 小时时分钟必须为 00。表示未知偏移的 `-00:00` 拒绝。数字范围、日期有效性及闰年规则严格校验，不允许越界自动进位。

带偏移输入已确定瞬间，禁止再用 `source_timezone` 覆盖。只有后续制品显式声明本地格式后才允许该字段；区域时间落入夏令时重复或跳跃区间时默认拒绝，不猜测早晚候选。未带版本 tzdb 的制品必须拒绝 IANA 区域标识，不能降级为固定偏移。

不支持闰秒、相对日期、自然语言、系统本地时区、用户自定义格式代码或取当前时间；未支持的格式与时区明确失败。小数秒按已声明精度保留，不隐式丢失精度。v1 一次返回完整 JSON，不流式。
