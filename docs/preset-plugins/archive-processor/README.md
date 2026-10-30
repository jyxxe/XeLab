# 归档处理预设规划

[预设目录](../README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：`planned`，尚未实现、构建或实测；计划业务契约版本：0.1.0。

插件目录：`archive-processor/`；拟定插件ID：`org.xelab.archive-processor`。创建或解压ZIP、TAR归档，限制解压总量、文件数量、路径深度和临时盘占用。

## 能力与依赖

| 项目 | 规划定义 |
|---|---|
| 能力ID | `archive.extract`、`archive.create` |
| 执行形态 | 普通OCI插件，独立CPU容器；不由微插件运行时执行 |
| 依赖 | 本目录 `contracts/`；受控核心Blob输入/导出；有界临时文件存储；ZIP/TAR处理库。 |
| 初始资源建议 | 2 vCPU / 1 GiB；单副本，仅为未实测起点，须经基准后冻结 |
| 文件边界 | 核心Blob输入/导出≤64MiB；不接收任意宿主路径 |
| JSON边界 | 业务输入≤4MiB、输出≤16MiB；另受逐接口更小限制 |

本版每个输入或输出Blob均≤64MiB；不假定分片上传、流式无限输出或跨调用持久工作目录。超过限制拒绝该单项。

## 权限与隔离

核心通过固定Skill转发到批准的安装和能力；每次验证Skill、底层能力、账户来源链及权限组上限。涉及Secret、Blob或网络时还需相应资源授权，不能让插件身份扩大调用者权限。

资源全部从普通插件池准入，不借用内核至少4C/16GiB预留；临时盘、进程数、活动并发与排队上限也需显式配置。Secret仅由核心绑定并按批准协议注入，不接收或回显裸密钥。

单次操作是有界工作，批量统一采用[核心异步batch](../../skills/batches.md)逐项调用；跨项不隐含事务。外部操作结果不明时不自动重发；具体副作用、恢复与重试按逐接口语义处理。

## 接口与待落地条件

- [逐接口定义](interfaces.md)
- `archive.extract`： [输入](contracts/extract/input.schema.json) · [输出](contracts/extract/output.schema.json) · [流](contracts/extract/stream.schema.json) · [语义](contracts/extract/semantics.json)
- `archive.create`： [输入](contracts/create/input.schema.json) · [输出](contracts/create/output.schema.json) · [流](contracts/create/stream.schema.json) · [语义](contracts/create/semantics.json)

Blob导出适配、归档格式和文件名规范、流式上限与临时文件回收；路径穿越和压缩炸弹测试。

以上是待实现设计条目；本目录不能直接作为安装包。未完成清单、适配、安全验收和契约验证前，不列入现有可部署 `catalog.presets` 或稳定原始契约注册表。

