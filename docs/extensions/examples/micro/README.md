# 微宿主扩展

[扩展示例目录](../README.md)

> 规格版本：2.8.0；仅结构样例，未实际运行。

新微版本显式声明只读扩展点；ASCII规则不取得网络、HostService或宿主模块权限。

| 文件 | 用途 |
|---|---|
| [host-manifest.json](host-manifest.json) | 显式宿主点声明 |
| [extension-manifest.json](extension-manifest.json) | 独立扩展清单 |
| [contribution.json](contribution.json) | 原始JSON贡献 |
| [artifact.json](artifact.json) | 真实贡献/包摘要 |
| [publish-version.json](publish-version.json) | 固定源码Blob发布请求 |
| [create-deployment.json](create-deployment.json) | 创建disabled绑定 |
| [identity-closure.json](identity-closure.json) | 稳定身份闭包 |
| [runtime-snapshot.json](runtime-snapshot.json) | 当前运行代次快照 |
| [candidate.skill.json](candidate.skill.json) | 核心候选结构 |
| [register-skill.json](register-skill.json) | 独立注册请求 |
| [invoke.json](invoke.json) | 业务调用输入 |

[新微宿主源码](host-source.js)及其[独立包摘要](host-artifact.json)用于演示MPR1.3明确声明的只读host.extensionData；不改写已有echo包。缺少对应运行时适配器时不得加载。

宿主image_digest与UUID为待替换值，不是已发布安装。业务结果需真实宿主实现验证，静态样例只验证来源、绑定和结构。
