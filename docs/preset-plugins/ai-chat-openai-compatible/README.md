# OpenAI 兼容聊天预设规划

[预设目录](../README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：`planned`，尚未实现、构建或实测；计划业务契约版本：0.1.0。

插件目录：`ai-chat-openai-compatible/`；拟定插件ID：`org.xelab.ai-chat-openai-compatible`。转发配置的外部文本模型 API，向平台提供普通文本响应和流式文本响应。

## 能力与依赖

| 项目 | 规划定义 |
|---|---|
| 能力ID | `ai.chat` |
| 执行形态 | 普通OCI插件，独立CPU容器；不由微插件运行时执行 |
| 依赖 | 本目录 `contracts/`；平台 Secret 绑定；按部署配置选择的外部模型 API；受控网络出口。 |
| 初始资源建议 | 1 vCPU / 512 MiB；单副本，仅为未实测起点，须经基准后冻结 |
| 文件边界 | 核心Blob输入/导出≤64MiB；不接收任意宿主路径 |
| JSON边界 | 业务输入≤4MiB、输出≤16MiB；另受逐接口更小限制 |

这里的兼容范围采用[文本 AI 子集](../../communication/ai-profile.md)，不据此承诺工具调用、多模态或其他第三方 SDK 字段。

## 权限与隔离

核心通过固定Skill转发到批准的安装和能力；每次验证Skill、底层能力、账户来源链及权限组上限。涉及Secret、Blob或网络时还需相应资源授权，不能让插件身份扩大调用者权限。

资源全部从普通插件池准入，不借用内核至少4C/16GiB预留；临时盘、进程数、活动并发与排队上限也需显式配置。Secret仅由核心绑定并按批准协议注入，不接收或回显裸密钥。

单次操作是有界工作，批量统一采用[核心异步batch](../../skills/batches.md)逐项调用；跨项不隐含事务。外部操作结果不明时不自动重发；具体副作用、恢复与重试按逐接口语义处理。

## 接口与待落地条件

- [逐接口定义](interfaces.md)
- `ai.chat`： [输入](contracts/chat/input.schema.json) · [输出](contracts/chat/output.schema.json) · [流](contracts/chat/stream.schema.json) · [语义](contracts/chat/semantics.json)

外部提供者适配、模型别名配置、流式取消和计量测试；批准 Secret 与目标地址绑定；将业务流适配到既有 Skill 流协议。

以上是待实现设计条目；本目录不能直接作为安装包。未完成清单、适配、安全验收和契约验证前，不列入现有可部署 `catalog.presets` 或稳定原始契约注册表。

