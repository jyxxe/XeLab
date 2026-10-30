# text.chunk接口草案

[插件规划](README.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/chunk/input.schema.json) |
| 输出 | [output.schema.json](contracts/chunk/output.schema.json) |
| 流式 | [stream.schema.json](contracts/chunk/stream.schema.json) |
| 语义 | [semantics.json](contracts/chunk/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `text` | 完整原文，严格Unicode字符串 |
| 输入 | `mode` | `characters`、`paragraphs`、`tokens`或`tokens-estimate`；省略按characters解释 |
| 输入 | `size` | 必填，1..1048576；每片最多单位数，字符/段落为码点，tokens为精确token，tokens-estimate为估算单位 |
| 输入 | `overlap` | 相邻片段重叠的同类单位数，0..size-1；省略按0解释，段落模式首版必须为0 |
| 输入 | `tokenizer_id` / `tokenizer_version` | tokens模式必须显式指定，并匹配已审核固定依赖 |
| 输入 | `estimator_id` / `estimator_version` | tokens-estimate模式可选；省略采用固定规划估算器utf8-div4-v1/0.1.0 |
| 输出 | `chunks` | 有序`{text,start,end}`数组，最多10000项 |
| 输出 | `source_length` | 原文Unicode码点总数 |
| 输出 | `mode` | 实际使用的切分模式 |
| 输出 | `estimated` | 仅tokens-estimate为true；其他三种模式为false |
| 输出 | `tokenizer_id` / `tokenizer_version` | tokens模式回显固定标识；其他模式省略 |
| 输出 | `estimator_id` / `estimator_version` | tokens-estimate回显固定估算器标识；其他模式省略 |

## 偏移与完整性

start/end始终是原文从0开始的Unicode码点偏移，采用左闭右开区间`[start,end)`，不因mode改变单位。每个chunk.text必须等于原文对应切片，文本使用UTF-8 JSON返回；不允许切开一个Unicode码点。

空原文返回空chunks且source_length=0。characters模式按size码点取片，以size-overlap推进；最后一片可短于size。每片非空且起点严格前进，避免尾端重复产生等价片段。

mode、overlap及估算器字段的省略行为由固定处理器实现，Schema的default注解不自动填参；不属于实际模式的Tokenizer或估算器字段均拒绝。

paragraphs模式把连续空行结束位置视为候选边界，保留全部分隔符。选不超过size码点的最远段落边界；单段超长时按码点硬切，overlap固定0。无重叠时按顺序拼接chunk.text必须严格还原原文。

tokens模式用指定Tokenizer编码全篇形成候选窗口，再映射为原文完整码点区间。实现须具有可核实的token到原文映射；不能用decode后变化的文本充当原文切片，非文本特殊token只计入固定计数规则，不伪造字符偏移。

每个最终原文切片必须作为独立输入，用同Tokenizer ID、版本及特殊token计数规则重新编码，验证精确数量不超过size。全篇token窗口的长度不能替代该校验，因为独立分词上下文可能改变结果。

若独立计数超限，适配器可沿合法码点边界缩短并重新计算后续窗口，但须同时保持原文完整覆盖、约定重叠、起点严格前进及单片上限。不能全部满足、没有无损映射或无法在预算内核实时，整次失败，不以估算结果替代tokens成功输出。

tokens-estimate按固定估算器处理，不加载未声明的Tokenizer。规划的utf8-div4-v1/0.1.0估算值为`ceil(UTF8字节数/4)`；取不超过4×size字节的最长完整码点前缀，再保留不超过4×overlap字节的最长完整码点后缀作为重叠。下一起点必须严格前进，最后片段不产生重复尾片。

该估算器尚未实现或完成效果标定。estimated=true不表示精确token上限，进入Embedding前必须按目标模型Tokenizer再校验；未知估算器、未经审核版本及不匹配模式字段整次拒绝，不回退为精确模式。

有重叠时按start/end去除重复覆盖后可还原原文；必须覆盖原文全部码点，不自动去掉空白。所有Schema及输出字节限额优先，超限整次失败。无流式结果；多文档走核心异步batch。
