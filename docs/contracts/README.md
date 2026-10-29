# 机器契约目录

[文档总目录](../README.md) · [维护流程](../implementation/change-policy.md)

本目录的长文件是机器契约；人工说明按主题和单接口拆分。所有接口为设计目标，尚未实现。

| 来源 | 内容 | 人工入口 |
|---|---|---|
| [openapi.json](openapi.json) | 174 个 HTTP 操作、204 个 Schema；结构、权限动作及目标 | [逐操作](../api/README.md) |
| [schemas/](schemas/) | 单模型 JSON Schema 投影，与 OpenAPI 一致 | [字段模型](../api/models/README.md) |
| [proto/xelab/v1/](proto/xelab/v1/) | 7 个 proto3 文件，33 个 RPC | [逐 RPC](../rpc/README.md) |
| [rpc-semantics.json](rpc-semantics.json) | 每个 RPC 的期限、对端、授权、幂等和校验 | [通用语义](../communication/grpc.md) |
| [internal.schema.json](internal.schema.json) | 内核进程内类型 | [模块接口](../internal/README.md) |
| [internal-interfaces.json](internal-interfaces.json) | 34 个内核稳定方法的签名/事务 | [模块接口](../internal/README.md) |
| [ui-bridge.schema.json](ui-bridge.schema.json) | iframe握手、请求/响应、主题/可见性事件 | [桥协议](../communication/ui-bridge.md) |
| [permissions.json](permissions.json) | 动作、允许资源类型与操作对应 | [权限目录](../identity/permissions.md) |
| [kernel-config.schema.json](kernel-config.schema.json) | TOML 解析后的配置字段与边界 | [配置](../operations/configuration.md) |
| [metadata.sql](metadata.sql) | PostgreSQL 表、唯一键、FK 与基础 CHECK | [事务规则](../operations/persistence.md) |
| [examples/](examples/README.md) | 可校验请求、清单、流程和配置样例 | [样例索引](examples/README.md) |

OpenAPI/JSON Schema 不替代跨对象状态和授权语义；SQL 语法解析不替代真实数据库迁移测试。Proto 标量必填及条件校验由接收方执行。

接口变更先修改本目录相应来源，再重新生成和校验；不要直接修改 api/ 或 rpc/ 的生成页。

预设业务契约：[共享目录](../preset-plugins/contracts/README.md)、[知识库](../preset-plugins/knowledge-base/interfaces.md)、[Skill定义](../skills/README.md)。这些接口由核心SkillRouter转发，不在公开网关额外开放插件内网端口。
