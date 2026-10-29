# 文本切分微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-text-chunk/` / `org.xelab.micro-text-chunk` |
| 能力ID | `text.chunk`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；固定文本分割规则；Tokenizer（精确token模式时） |

按字符、段落边界、精确token或显式token估算规则生成原文片段，并保留Unicode码点偏移，供Embedding和知识库流程显式调用。切分本身不写入知识库、不创建向量，也不选择用户未授权的Embedding接口。

能力ID仅为规划索引。未来固定微版本通过审核并创建MicroDeployment后，核心才生成候选Skill。

## 切分与Tokenizer门槛

字符与段落规则可由有界纯JavaScript实现。字符指Unicode码点，不是UTF-16代码单元；返回片段始终来自原文，不自动trim、归一化或改换行。

精确token模式要求显式Tokenizer ID和版本，并将词表、配置与依赖摘要绑定到不可变微版本。Runtime须具备审核过的分词库和可核实的token到原文边界映射；每个最终切片还须独立重新编码验证上限，否则该模式不可启用。

规划的tokens-estimate模式使用固定估算器并强制输出estimated=true；tokens模式仍专指精确计数。默认规划估算器为utf8-div4-v1/0.1.0，按UTF-8字节数除4向上取整，尚未实现或完成效果标定。

估算不保证任何真实模型的token上限，送入Embedding前仍须使用目标模型的真实Tokenizer重新校验。片段数量、重叠和总输出大小必须预估并持续计量，不把近似结果标为精确。

## 安全预算

输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。最多10000片段；重叠不能造成无限循环或突破输出预算。

默认并发1，资源归父Runtime池，不独立创建容器，不占用内核至少4C16G预留。VM仅虚拟stdin/stdout/stderr，无文件、网络、Secret、进程和原始HostService访问。

多文档使用[核心异步batch](../../skills/batches.md)，每项提交一段完整有界文本。结果超限应缩小输入后另行调用，不能以静默截断冒充完整切分。
