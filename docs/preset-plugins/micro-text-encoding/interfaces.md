# text.encoding-convert接口草案

[插件规划](README.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/encoding-convert/input.schema.json) |
| 输出 | [output.schema.json](contracts/encoding-convert/output.schema.json) |
| 流式 | [stream.schema.json](contracts/encoding-convert/stream.schema.json) |
| 语义 | [semantics.json](contracts/encoding-convert/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `data` | 标准base64字节，严格检查字母表、padding和尾位 |
| 输入 | `from` / `to` | `utf-8`、`utf-16le`或`utf-16be`，不自动检测或接受别名 |
| 输入 | `errors` | `strict`或`replace`；省略按strict解释 |
| 输入 | `bom` | 目标`omit`或`emit`；省略按omit解释 |
| 输出 | `data` | 完整转换后字节的标准base64 |
| 输出 | `encoding` | 实际目标编码，等于输入to |
| 输出 | `replacement_count` | 解码期间新增U+FFFD的次数；原文已有U+FFFD不计数 |

## 解码、BOM与错误

源编码必须显式声明。输入开头若存在与from一致的BOM，则恰好去掉一次；与from冲突的已知BOM使整次失败。其他位置的U+FEFF作为文本保留，不当作BOM删除。

UTF-8采用严格标量值规则，拒绝过长序列、孤立续字节、代理项和超范围码点；UTF-16按声明端序解码，拒绝奇数字节及不成对代理项。strict遇到第一处错误终止，不返回部分转换结果。

replace模式按Unicode最大子部分规则将每个非法子序列替换为一个U+FFFD并计数；UTF-16末尾单字节替换一次。该规则必须与锁定Codec的测试向量一致，不能由实现自行选择吞掉任意后续字节。

目标UTF编码均可表示合法Unicode标量。emit在结果前添加恰好一个目标BOM，omit不添加；数据中本来存在的非首位U+FEFF继续保留。空输入在omit时输出空字节，在emit时只输出目标BOM。

不支持的编码、非法base64、冲突BOM、Codec不可用、超限均整次失败。default注解不自动填参，省略行为须由处理器实现。无流式输出；多项转换使用核心异步batch。
