# 受控 HTTP 客户端预设规划

[预设目录](../README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：`planned`，尚未实现、构建或实测；计划业务契约版本：0.1.0。

插件目录：`http-client/`；拟定插件ID：`org.xelab.http-client`。向获批目标发起通用 HTTP 请求，统一执行超时、响应大小、地址和网络访问策略。

## 能力与依赖

| 项目 | 规划定义 |
|---|---|
| 能力ID | `http.request` |
| 执行形态 | 普通OCI插件，独立CPU容器；不由微插件运行时执行 |
| 依赖 | 本目录 `contracts/`；平台网络访问策略与审批；DNS/TLS验证；可选 Secret 认证绑定；核心 Blob。 |
| 初始资源建议 | 1 vCPU / 256 MiB；单副本，仅为未实测起点，须经基准后冻结 |
| 文件边界 | 核心Blob输入/导出≤64MiB；不接收任意宿主路径 |
| JSON边界 | 业务输入≤4MiB、输出≤16MiB；另受逐接口更小限制 |

它是获批范围内的通用 HTTP 普通插件；[微插件运行时](../micro-plugin-runtime/README.md)的固定模板 ProxyHttp 仍保持自己的受限边界，两者不互相替代。

## 权限与隔离

核心通过固定Skill转发到批准的安装和能力；每次验证Skill、底层能力、账户来源链及权限组上限。涉及Secret、Blob或网络时还需相应资源授权，不能让插件身份扩大调用者权限。

资源全部从普通插件池准入，不借用内核至少4C/16GiB预留；临时盘、进程数、活动并发与排队上限也需显式配置。Secret仅由核心绑定并按批准协议注入，不接收或回显裸密钥。

单次操作是有界工作，批量统一采用[核心异步batch](../../skills/batches.md)逐项调用；跨项不隐含事务。外部操作结果不明时不自动重发；具体副作用、恢复与重试按逐接口语义处理。

## 接口与待落地条件

- [逐接口定义](interfaces.md)
- `http.request`： [输入](contracts/request/input.schema.json) · [输出](contracts/request/output.schema.json) · [流](contracts/request/stream.schema.json) · [语义](contracts/request/semantics.json)

通用HTTP出口审批模型、逐跳地址校验、响应导出与凭证注入适配；验证DNS重绑定、重定向和取消。

以上是待实现设计条目；本目录不能直接作为安装包。未完成清单、适配、安全验收和契约验证前，不列入现有可部署 `catalog.presets` 或稳定原始契约注册表。

