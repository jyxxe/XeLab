# 扩展宿主链

[扩展示例目录](../README.md)

> 规格版本：2.8.0；仅结构样例，未实际运行。

固定ordinary示例的扩展部署为宿主，调用自己的text.handler点；处理器Skill与其资源另行授权。

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

[固定处理器定义](handler.skill.json)来自已有纯微echo样例；清单中的handler_definition_digest与该文件规范JSON一致。处理器业务输入保持point input，贡献/config不偷偷注入处理器参数；父点如何使用配置由已发布适配器定义。

宿主image_digest与UUID为待替换值，不是已发布安装。业务结果需真实宿主实现验证，静态样例只验证来源、绑定和结构。
