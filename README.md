# Xelab

单节点通用插件平台，包含 2.8.0 架构/接口规格及 M1–M4 范围内的普通 OCI、身份、存储、Skill/WebUI、依赖与事件实现。

实现范围、构建运行和测试见 [Kernel README](crates/kernel/README.md)。M4 的功能、恢复与验收边界见 [实现记录](docs/implementation/m4-acceptance.md)，精确提交、阶段 PR 及最终 main CI 由 [目标 #51](https://github.com/jyxxe/XeLab/issues/51) 维护。正式生产验收由 [#50](https://github.com/jyxxe/XeLab/issues/50) 独立跟踪，AWS 继续 DEFERRED；不代表完整 2.8 平台已交付。

从 [docs/README.md](docs/README.md) 阅读规格。接口机器契约位于 [docs/contracts](docs/contracts/README.md)，实施与验收位于 [docs/implementation](docs/implementation/README.md)。

参与开发见 [贡献指南](CONTRIBUTING.md) 与 [Issue 提案流程](docs/implementation/issue-workflow.md)；安全漏洞请按 [安全报告政策](SECURITY.md)处理。

