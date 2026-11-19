# Xelab

单节点通用插件平台，包含 2.8.0 架构/接口规格及 M1–M4 范围内的普通 OCI、身份、存储、Skill/WebUI、依赖与事件实现。

实现范围、构建运行和测试见 [Kernel README](crates/kernel/README.md)。M4 的功能、恢复与验收边界见 [实现记录](docs/implementation/m4-acceptance.md)。历史上游 `jyxxe/XeLab` 的 [目标 #51](https://github.com/jyxxe/XeLab/issues/51) 和[生产验收 #50](https://github.com/jyxxe/XeLab/issues/50) 是原始证据，不是同号的 Forgejo 工单；AWS 继续 DEFERRED，不代表完整 2.8 平台已交付。

日常开发以本地 Forgejo `xelab-admin/XeLab` 的 `dev` 为准。原开发仓库 `jyxxenon/XeLab` 的工单见[旧号到 Forgejo 的对照表](http://127.0.0.1:3000/xelab-admin/XeLab/issues/5#issuecomment-141)；恢复记录不等于当前开发待办。迁移源码核对步骤见[开发工具](tools/dev/README.md)。

从 [docs/README.md](docs/README.md) 阅读规格。接口机器契约位于 [docs/contracts](docs/contracts/README.md)，实施与验收位于 [docs/implementation](docs/implementation/README.md)。

参与开发见 [贡献指南](CONTRIBUTING.md) 与 [Issue 提案流程](docs/implementation/issue-workflow.md)；安全漏洞请按 [安全报告政策](SECURITY.md)处理。

