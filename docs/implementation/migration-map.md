# 原计划迁移映射

[文档总目录](../README.md) · [原始1.2归档](../archive/v0-1.2.md)

原文已完整归档；当前规范改为按职责维护。以下每章都有主落点，关联接口通过各主题交叉链接。

| 原章节 | 当前主入口 |
|---|---|
| 1 项目目标 | [architecture/scope.md](../architecture/scope.md) |
| 2 总体架构 | [architecture/components.md](../architecture/components.md) |
| 3 领域模型 | [architecture/domain.md](../architecture/domain.md) |
| 4 插件类型 | [plugins/README.md](../plugins/README.md) |
| 5 清单与契约 | [plugins/manifest.md](../plugins/manifest.md) |
| 6 API兼容 | [communication/http.md](../communication/http.md) |
| 7 能力注册 | [plugins/dependencies.md](../plugins/dependencies.md) |
| 8 依赖解析 | [plugins/dependencies.md](../plugins/dependencies.md) |
| 9 容器与资源 | [runtime/budgets.md](../runtime/budgets.md) |
| 10 数据卷 | [runtime/volumes.md](../runtime/volumes.md) |
| 11 工作流 | [tasks/README.md](../tasks/README.md) |
| 12 通信协议 | [communication/grpc.md](../communication/grpc.md) |
| 13 安全与权限 | [identity/README.md](../identity/README.md) |
| 14 生命周期 | [plugins/lifecycle.md](../plugins/lifecycle.md) |
| 15 恢复与观测 | [operations/observability.md](../operations/observability.md) |
| 16 技术选型 | [architecture/scope.md](../architecture/scope.md) |
| 17 部署扩展 | [operations/deployment.md](../operations/deployment.md) |
| 18 验收 | [implementation/acceptance.md](../implementation/acceptance.md) |
| 19 实施计划 | [implementation/roadmap.md](../implementation/roadmap.md) |
| 20 总结 | [README.md](../README.md) |

新增细化：制品/部署/实例状态分离、崩溃对账、单写卷升级、结果未知、可信认证页、源凭证吊销和版本化权限来源，都是原目标的可执行约束。HTTP所有操作和RPC/内核模块均另有逐接口定义。
