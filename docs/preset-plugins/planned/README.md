# 新增插件规划

[预设插件目录](../README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。定义待实现的接口，不表示已有服务或可安装制品。

本次新增 **12 个普通插件、20 个微插件、37 个能力入口**。每个插件一个子目录；多能力插件在 `contracts/<operation>/` 下分别维护字段与执行语义。

| 阅读目标 | 入口 |
|---|---|
| 普通插件的目录、能力、依赖、用途 | [普通插件目录](ordinary.md) |
| 微插件的目录、能力、依赖、用途 | [微插件目录](micro.md) |
| 缺少的库、核心适配、运行时能力 | [落地门槛](feature-gates.md) |
| 授权、限额、错误、复用和重试 | [共享接口规则](interface-rules.md) |
| 转发为 Skill、batch 及发布条件 | [Skill规划](../../skills/planned/README.md) |
| 分阶段实施与验收 | [实施顺序](delivery.md) |
| 机器可读的插件与依赖清单 | [planned-catalog.json](../planned-catalog.json) |
| 按能力索引的原始接口、版本、摘要 | [planned-contracts.json](../planned-contracts.json) |
| 按能力阅读所有输入输出字段 | [37个逐接口页面](contracts.md) |
| 共享 JSON Pointer、条件 AST、XML 树类型 | [shared.schema.json](shared.schema.json) |
| 结构正反样例 | [validation.json](validation.json) |

## 编目与发布边界

现有 [catalog.json](../catalog.json) 的 `presets` 仍为7个OCI预设，`micro_presets` 仍为网页搜索1个微预设；`planned_catalog` 指向新增规划。规划不进入这两个安装列表，也不生成虚假镜像、微源码、制品摘要、部署 UUID 或可调用 Skill。

`capability_id` 保留需求中的名称，用于业务能力发现；它不是核心 HTTP 路径，也不是调用时可替换的插件地址。正式发布时将其与具体实现、契约版本和摘要绑定：普通OCI核验镜像签名及来源，微制品核验独立的所有者、源码和包摘要，MPR容器签名不能替代微制品来源验证。多个实现不以这个名称共享资源或绕过授权。

普通插件最终以独立 CPU OCI 容器交付。微插件最终以独立的核心 MicroPlugin/Version/Deployment 对象交付，依赖 MPR 容器执行；不冒充 OCI 安装对象，也不获得 MPR 本身的权限。普通插件的资源须经 CPU 实测制定，微插件不再单独预留容器配额。

## 契约维护

每个能力均包含 `input.schema.json`、`output.schema.json`、`stream.schema.json`、`semantics.json`，并有自动生成的逐字段页面。`stream` 为 `null` 类型时只表示不支持流；它不是流中可发送的业务帧。只有 `ai.chat` 定义业务流。

四文件摘要按共享契约的文件清单算法计算，引用的共享 Schema/OpenAPI另列引用文件 SHA-256。草案引用当前文档，发布前须将引用闭包打包或内联为运行时可接受的 Schema 并重算正式摘要。共享 `$ref` 不能直接塞入当前不支持引用的 MicroBusinessSchema。

Schema 的结构约束与业务语义共同生效：字节预算、AST深度、动态字段对应、Unicode处理、DNS检查和外部执行证据需要实现时验证。静态样例通过不代表运行时、网络服务、沙箱或实际算法已通过测试。
