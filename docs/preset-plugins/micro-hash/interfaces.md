# crypto.hash接口草案

[插件规划](README.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。接口尚未实现。

| 契约 | 文件 |
|---|---|
| 输入 | [input.schema.json](contracts/hash/input.schema.json) |
| 输出 | [output.schema.json](contracts/hash/output.schema.json) |
| 流式 | [stream.schema.json](contracts/hash/stream.schema.json) |
| 语义 | [semantics.json](contracts/hash/semantics.json) |

## 业务字段

| 方向 | 字段 | 定义 |
|---|---|---|
| 输入 | `algorithm` | 可省略，按固定`sha256`解释 |
| 输入 | `input_encoding` | `text`或`base64`；省略按text解释 |
| 输入 | `data` | 对应编码的完整字符串，受1MiB JSON总预算约束 |
| 输出 | `algorithm` | 固定sha256 |
| 输出 | `digest` | `sha256:`前缀加64位小写十六进制摘要，共71字符 |
| 输出 | `bytes` | 实际进入哈希计算的字节数 |

## 字节语义

text模式将解码后的JSON字符串按严格UTF-8编码。拒绝孤立代理项，不添加BOM，不做Unicode规范化、去空白或换行转换。语义相近但字节不同的字符串可有不同摘要。

省略字段的行为由固定版本处理器显式实现，Schema的default注解不自动修改输入。data必填，空字符串合法。

base64模式只接受标准字母表、规范padding及完整四字符组；拒绝空白、URL安全变体、非法尾位与无效padding。空字符串表示零字节。摘要针对解码后的字节，bytes不是base64字符数。

算法不支持时明确失败，不自动降级为MD5或其他摘要。非法编码、库不可用、期限或内存越限均整次失败；不会为输入前缀、部分结果或缺失文件返回正常digest。

输入封装大小在解码前检查，解码和实现内存另受heap预算约束。输出预算包含完整JSON。stdout只提交完整结果，stream契约禁用。

本接口无file_id、URL、host_path、secret或分块续传句柄。大文件能力必须另过受控流协议门槛；核心batch只组织多次独立计算，不改变单次字节语义。
