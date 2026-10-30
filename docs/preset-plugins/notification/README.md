# 通知编排预设规划

[预设目录](../README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：`planned`，尚未实现、构建或实测；计划业务契约版本：0.1.0。

插件目录：`notification/`；拟定插件ID：`org.xelab.notification`。统一选择通知渠道与路由，执行限流、去重和受控失败重试；邮件通过 email.send 发送。

## 能力与依赖

| 项目 | 规划定义 |
|---|---|
| 能力ID | `notification.send` |
| 执行形态 | 普通OCI插件，独立CPU容器；不由微插件运行时执行 |
| 依赖 | 本目录 `contracts/`；平台 Secret 绑定；邮件渠道的 `email.send`；Webhook渠道的获批HTTP服务；有界调用与持久幂等记录。 |
| 初始资源建议 | 1 vCPU / 256 MiB；单副本，仅为未实测起点，须经基准后冻结 |
| 文件边界 | 核心Blob输入/导出≤64MiB；不接收任意宿主路径 |
| JSON边界 | 业务输入≤4MiB、输出≤16MiB；另受逐接口更小限制 |

通知插件负责路由和编排；邮件协议与提供者传输归[email-connector](../email-connector/README.md)，不在本插件中重复实现。

v0.1.0在根deadline内返回上游明确结果或unknown；accepted表示上游明确接收，delivered仅限上游提供同步送达终态证据。大量请求由核心异步batch排队；已完成的Call结果不会在后台继续从accepted改为delivered。

## 权限与隔离

核心通过固定Skill转发到批准的安装和能力；每次验证Skill、底层能力、账户来源链及权限组上限。涉及Secret、Blob或网络时还需相应资源授权，不能让插件身份扩大调用者权限。

资源全部从普通插件池准入，不借用内核至少4C/16GiB预留；临时盘、进程数、活动并发与排队上限也需显式配置。Secret仅由核心绑定并按批准协议注入，不接收或回显裸密钥。

单次操作是有界工作，批量统一采用[核心异步batch](../../skills/batches.md)逐项调用；跨项不隐含事务。外部操作结果不明时不自动重发；具体副作用、恢复与重试按逐接口语义处理。

## 接口与待落地条件

- [逐接口定义](interfaces.md)
- `notification.send`： [输入](contracts/send/input.schema.json) · [输出](contracts/send/output.schema.json) · [流](contracts/send/stream.schema.json) · [语义](contracts/send/semantics.json)

渠道路由配置、对子调用的来源链传播、持久去重记录与根deadline内的受控重试；核实各提供者的去重和结果查询能力。真正后台通知队列、投递查询或回调须单独冻结gate/API及授权续期机制，不能借旧用户token无限后台运行。

以上是待实现设计条目；本目录不能直接作为安装包。未完成清单、适配、安全验收和契约验证前，不列入现有可部署 `catalog.presets` 或稳定原始契约注册表。

