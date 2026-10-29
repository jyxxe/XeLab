# Xelab 单节点插件平台规格

> 规格版本 **2.6.0**。本目录定义待实现的系统，不表示已有运行服务。现行规格替代原 plan/v0.md，原文完整保存在 archive。

平台采用微内核、独立 CPU 容器、能力契约；单节点 Linux；内核至少预留 **4 vCPU / 16 GiB**，宿主基础设施另行预留；WebUI 为独立插件。角色包含 Guest，支持版本化权限组、多级子账户及派生 API Key。

## 阅读入口

| 工作 | 入口 |
|---|---|
| 理解边界与模块 | [范围](architecture/scope.md)、[架构](architecture/components.md)、[对象](architecture/domain.md) |
| 实现一个 HTTP 操作 | [逐接口目录](api/README.md)、[字段模型](api/models/README.md)、[公共约定](communication/http.md) |
| 选用/开发预设插件 | [预设目录](preset-plugins/README.md)、[embedding](preset-plugins/embedding-gemma-model.md)、[知识库](preset-plugins/knowledge-base/README.md) |
| 查看新增插件规划 | [普通插件12个](preset-plugins/planned/ordinary.md)、[微插件20个](preset-plugins/planned/micro.md)、[37个能力接口](preset-plugins/planned/contracts.md)、[落地门槛](preset-plugins/planned/feature-gates.md) |
| 定义业务Skill与转发 | [Skill层](skills/README.md)、[核心调用](api/operations/skills/README.md)、[知识库接口](skills/knowledge-base/README.md) |
| 执行异步batch | [提交与查询](skills/batches.md)、[调度与恢复](tasks/batches.md)、[逐接口](api/operations/batches/README.md) |
| 实现桌面和插件页面 | [WebUI桌面](preset-plugins/webui/README.md)、[窗口/组件声明](plugins/ui-contributions.md)、[消息桥](communication/ui-bridge.md) |
| 实现 Rust/C++ 插件 | [清单](plugins/manifest.md)、[RPC方法](rpc/README.md)、[可校验样例](contracts/examples/README.md) |
| 实现用户权限 | [身份权限](identity/README.md)、[动作目录](identity/permissions.md)、[继承](identity/inheritance.md) |
| 实现运行/恢复 | [资源](runtime/README.md)、[对账](architecture/reconciliation.md)、[任务](tasks/README.md) |
| 部署或排障 | [配置](operations/configuration.md)、[CLI](operations/cli.md)、[运维](operations/README.md) |
| 开始开发/验收 | [里程碑](implementation/roadmap.md)、[验收](implementation/acceptance.md)、[变更规则](implementation/change-policy.md) |

## 目录结构

专题索引：[预设插件](preset-plugins/README.md) · [Skill](skills/README.md) · [架构](architecture/README.md) · [插件](plugins/README.md) · [身份权限](identity/README.md) · [运行资源](runtime/README.md) · [任务](tasks/README.md) · [通信](communication/README.md) · [运维](operations/README.md) · [实施验收](implementation/README.md) · [历史归档](archive/README.md)。

```text
docs/
├── architecture/       范围、领域对象、状态机、对账
├── preset-plugins/     已有预设及32个新增规划，每插件独立目录；planned/分类索引与落地门槛
├── skills/             版本化业务入口、转发、权限及逐Skill文档
├── plugins/            清单、制品、依赖、生命周期、迁移、WebUI
├── identity/           角色、Guest、组、账户树、凭证、撤权、配额
├── runtime/            节点预算、隔离、卷、密钥
├── tasks/              DAG、尝试/租约、恢复、定时、事件
├── communication/      HTTP、RPC、错误、幂等、SSE、兼容子集
├── api/operations/     按领域及单个 operationId 拆分
├── api/models/         按单个类型拆分字段与约束
├── rpc/                按单个 RPC 拆分字段与执行语义
├── internal/           内核进程内稳定接口
├── contracts/          OpenAPI、JSON Schema、Proto、SQL、样例
├── operations/         配置、部署、CLI、持久化、备份、观测
├── implementation/     阶段、验收、迁移映射、变更策略
├── tools/              文档生成与验证
└── archive/            原方案归档
```

## 契约规模与维护

HTTP **174** 个操作、**204** 个数据模型；插件/宿主 **33** 个 RPC；内核协作 **34** 个稳定方法。每个 HTTP 操作都有参数、请求/响应模型、授权目标、错误、并发和幂等规则。

机器契约和主题语义共同构成规范；示例不是额外字段声明，历史归档不参与当前接口定义。预设提供24个静态业务Skill；知识库字段索引默认128维，embedding为可选依赖。未来第三方业务接口由插件发布自己的契约，不能假定核心预先定义其全部业务对象。

另有12个普通插件、20个微插件的37个能力草案，按插件和操作分拆输入/输出/流/执行语义及逐字段文档；状态为planned，尚未注册为正式预设或Skill。微插件复用MPR容器，普通插件独立CPU容器，资源统一从插件池分配。

batch通常异步提交、查询进度和领取逐项结果。WebUI预设使用类似macOS的桌面交互与可变主题；每个插件可选提供独立iframe窗口和/或桌面组件，业务经核心Skill层转发。

校验入口：[工具说明](tools/README.md)。原章节对应：[迁移映射](implementation/migration-map.md)。外部规范：[参考来源](implementation/references.md)。
