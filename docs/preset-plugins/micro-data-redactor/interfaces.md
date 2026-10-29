# data.redact接口草案

[插件规划](README.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/redact/input.schema.json) |
| 输出 | [output.schema.json](contracts/redact/output.schema.json) |
| 流式 | [stream.schema.json](contracts/redact/stream.schema.json) |
| 语义 | [semantics.json](contracts/redact/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `data` | 待处理JSON值；处理过程不修改其他Call状态 |
| 输入 | `rules` | 有序规则数组，1..128条 |
| 输入规则 | `path` | 单个JSON Pointer，无通配符或递归路径 |
| 输入规则 | `action` | `replace`、`mask`或`regex-replace` |
| 输入规则 | `replacement` | replace使用任意受限JSON值；regex-replace使用纯字面量字符串 |
| 输入规则 | `keep_start` / `keep_end` / `mask` | mask保留的Unicode码点数与单码点遮罩字符；省略分别按0、0、`*`处理 |
| 输入规则 | `pattern` | regex-replace的受限模式，最多1024字符 |
| 输出 | `data` | 完整处理后的JSON值 |
| 输出 | `applied` | 规则序号、是否存在目标及替换次数；不含原始值或命中片段 |

## 规则语义

按数组顺序对当前结果求值；路径缺失跳过并记录未命中，路径语法非法使整次失败。空Pointer可定位根值。数组下标只替换该位置，首版无删除和数组压缩，防止后续规则下标漂移。

replace可覆盖任意类型。mask与regex-replace只接受字符串目标；类型不符整次失败。mask按Unicode码点计数，保留区间重叠时只保留原字符串一次；其余每码点替换为一个mask字符，不按UTF-16代码单元处理。

keep_start/keep_end各为0..1048576；default注解不自动填参，省略行为由固定处理器实现。每种action只接收对应规则字段，混用pattern、mask或其他action的参数均拒绝。

regex-replace最多替换10000次，使用审核的线性或等价受限引擎；不允许反向引用、递归模式或任意回调。replacement始终按字面量处理，不解释`$1`或代码；零宽匹配须按引擎明确规则向前推进并计入预算。

模式编译失败、引擎不可用、未知规则、越限或输出超限均整次失败。错误可报告规则下标和固定原因，不回显pattern中捕获的输入内容。无成功流式输出，不以部分脱敏结果冒充完整完成。

批量通过核心异步batch提交。输出及诊断继续受原Call授权控制，不因已执行脱敏而自动变为可公开数据。
