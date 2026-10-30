# 受限正则提取接口草案

[插件说明](README.md) · [微执行协议](../micro-plugin-runtime/execution.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。

## `text.regex-extract` / `regex-extract`

[输入 Schema](contracts/regex-extract/input.schema.json) · [输出 Schema](contracts/regex-extract/output.schema.json) · [流式声明](contracts/regex-extract/stream.schema.json) · [行为契约](contracts/regex-extract/semantics.json)

| 输入字段 | 类型与语义 |
|---|---|
| `text` | 必填字符串，UTF-8 最多 256KiB |
| `pattern` | 必填非空模式字符串，UTF-8 最多 4096B |
| `flags` | 可选字符串，默认空；仅允许不重复的 `i`、`m`、`s` |
| `max_matches` | 可选整数，默认 100、范围 1..1000 |

输出 `{matches: array, truncated: boolean}`。每项为 `{value,groups,start,end}`：`value` 是完整匹配，`groups` 按捕获编号返回字符串或 null，最多 128 个；`start` 为包含的起点，`end` 为不包含的终点，均按原文本 Unicode 标量编号。

匹配从左向右、非重叠，始终使用 Unicode 语义；没有用户可选的全局或粘连模式。零长度匹配后按一个 Unicode 标量前进，防止无限循环。模式编译失败、未支持语法或 flags 必须明确失败；禁止静默改变语义。

达到 `max_matches` 后检测是否仍存在后续匹配；存在时 `truncated=true`。该标记只表示条数限制，不掩盖超时或内存失败。匹配与捕获组共用输出 1MiB 限制；若即使有限条数也超限，则整体失败，不截断字符串。

模式编译、Unicode 索引转换和匹配均计入 2s 默认总期限。拒绝未配对代理码位。引擎的语法子集、Unicode 版本和资源终止方式必须随制品固定；无受审实现时接口保持计划状态。v1 非流式。
