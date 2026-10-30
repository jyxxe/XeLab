# Webhook 触发器预设规划

[预设目录](../README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：`planned`，尚未实现、构建或实测；计划业务契约版本：0.1.0。

插件目录：`automation-webhook-trigger/`；拟定插件ID：`org.xelab.automation-webhook-trigger`。把经过认证与防重放检查的入站Webhook映射为平台事件或持久任务触发。

## 能力与依赖

| 项目 | 规划定义 |
|---|---|
| 能力ID | `trigger.webhook` |
| 执行形态 | 普通OCI插件，独立CPU容器；不由微插件运行时执行 |
| 依赖 | 本目录 `contracts/`；API兼容层的新入站网关适配；平台任务或事件接口；平台Secret及重放记录。 |
| 初始资源建议 | 1 vCPU / 256 MiB；单副本，仅为未实测起点，须经基准后冻结 |
| 文件边界 | 核心Blob输入/导出≤64MiB；不接收任意宿主路径 |
| JSON边界 | 业务输入≤4MiB、输出≤16MiB；另受逐接口更小限制 |

这是规划中的外部入口，不代表普通Skill调用已经能够接收任意公有HTTP请求。签名、原始字节认证与限流必须在触发副作用之前完成。

## 权限与隔离

核心通过固定Skill转发到批准的安装和能力；每次验证Skill、底层能力、账户来源链及权限组上限。涉及Secret、Blob或网络时还需相应资源授权，不能让插件身份扩大调用者权限。

资源全部从普通插件池准入，不借用内核至少4C/16GiB预留；临时盘、进程数、活动并发与排队上限也需显式配置。Secret仅由核心绑定并按批准协议注入，不接收或回显裸密钥。

单次操作是有界工作，批量统一采用[核心异步batch](../../skills/batches.md)逐项调用；跨项不隐含事务。外部操作结果不明时不自动重发；具体副作用、恢复与重试按逐接口语义处理。

## 接口与待落地条件

- [逐接口定义](interfaces.md)
- `trigger.webhook`： [输入](contracts/webhook/input.schema.json) · [输出](contracts/webhook/output.schema.json) · [流](contracts/webhook/stream.schema.json) · [语义](contracts/webhook/semantics.json)

BLOCKED：`trusted-webhook-ingress` gate要求先冻结公有签名HTTP入口、保留路径路由、签名认证和可信来源上下文。事件模式可复用既有HostService.PublishEvent，但仍受该入口gate和发布授权约束。

任务模式另受`host-task-submission` gate阻塞：HostService尚无CreateTask接口，必须先定义受控任务提交协议与来源链/幂等语义，不能借现有用户HTTP接口绕过。

以上是待实现设计条目；本目录不能直接作为安装包。未完成清单、适配、安全验收和契约验证前，不列入现有可部署 `catalog.presets` 或稳定原始契约注册表。

