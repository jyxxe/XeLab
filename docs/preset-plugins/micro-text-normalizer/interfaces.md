# 文本规范化接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。

## `text.normalize` / `normalize`

[输入 Schema](contracts/normalize/input.schema.json) · [输出 Schema](contracts/normalize/output.schema.json) · [流式声明](contracts/normalize/stream.schema.json) · [行为契约](contracts/normalize/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `text` | 必填字符串 |
| `unicode_form` | 可选 `none` / `NFC` / `NFD` / `NFKC` / `NFKD`，默认 `none` |
| `normalize_newlines` | 可选布尔值，默认 true；CRLF 与单独 CR 转成 LF |
| `trim` | 可选布尔值，默认 false；移除整个文本首尾的 ASCII 空白 |
| `collapse_spaces` | 可选布尔值，默认 false；连续 ASCII 空格和 tab 折叠为一个空格 |

输出为 `{text: string}`，不附加隐式清理或补全。

固定执行顺序：Unicode 规范化、换行归一、连续空格折叠、首尾清理。ASCII 首尾空白明确为 U+0009..U+000D 与 U+0020；折叠仅作用于 U+0009 / U+0020，不删除换行、不把所有 Unicode 空白改为空格。

不做大小写转换、去除标点或去掉零宽字符；这些规则需未来独立声明。未提供所选 Unicode 形式的实现必须明确失败，不能悄悄退化为 `none`。拒绝包含未配对代理码位的字符串，避免不同实现产生不一致替换。

结果按完整 UTF-8 JSON 计量；规范化膨胀超过 1MiB 或内存/时间预算时整体失败。空字符串返回空字符串。v1 一次返回完整 JSON，不能用分片独立规范化后拼接来改变组合字符语义。
