# 文本编码转换微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-text-encoding/` / `org.xelab.micro-text-encoding` |
| 能力ID | `text.encoding-convert`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；审核过的字符编码转换库 |

接收base64字节和明确的源/目标编码，输出转换后字节的base64表示。转换不进行自动编码侦测、语言识别、Unicode规范化或换行清理。

能力ID为规划索引。未来固定微版本经库审核并创建MicroDeployment后，核心生成候选Skill；发布文档不代表当前Runtime已有编码库。

## Codec门槛

首版草案范围为UTF-8、UTF-16LE、UTF-16BE，要求固定端序、BOM及非法序列处理规则，并通过标准与异常字节测试。不能直接借用未审核的宿主TextDecoder或动态安装依赖。

GB18030、Big5及其他传统编码须有单独的映射表、版本、不可表示字符策略和内存成本评估后扩充契约。不得遇到不支持的编码就按UTF-8尝试并返回貌似成功的文本。

binary数据不适用编码转换。strict与replace行为显式区分，调用者可通过替换统计决定是否接受有损结果。

## 安全预算

输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。base64额外开销和目标编码膨胀均计入预算，结果不截断。

默认并发1，资源从父Runtime池扣除，无独立容器，不占用内核至少4C16G预留。VM仅虚拟stdin/stdout/stderr，无文件、网络、Secret、进程或原始HostService访问。

多个小文本使用[核心异步batch](../../skills/batches.md)。跨块多字节状态不在独立item间共享；大文件转换需未来受控流协议。
