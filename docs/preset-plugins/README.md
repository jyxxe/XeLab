# 预设插件目录

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：设计预设，尚未构建或实测运行。

每个插件占用一个独立子目录，包含说明、配置、接口/Skill、资源及验收。contracts是共享契约目录，不作为插件。通用开发规则仍在 [plugins](../plugins/README.md)，避免复制大段人工说明。

新增[12个普通插件与20个微插件规划](planned/README.md)，包含[37个逐能力接口](planned/contracts.md)。规划与下表已有预设分开编目，依赖、库及核心适配尚待实现；[普通插件](planned/ordinary.md)和[微插件](planned/micro.md)分别提供清晰的目录/能力/依赖/用途索引。

| 插件 | ID | CPU默认 | 内存默认 | 实例活动并发 |
|---|---|---|---|---|
| [web-search](web-search/README.md) | `org.xelab.web-search`（微预设） | 共享MPR | 受微限额约束 | 默认1、硬4 |
| [object-storage](object-storage/README.md) | `org.xelab.object-storage` | 1 vCPU | 1 GiB | 4 |
| [micro-plugin-runtime](micro-plugin-runtime/README.md) | `org.xelab.micro-plugin-runtime` | 1 vCPU | 1 GiB | 4 |
| [webui](webui/README.md) | `org.xelab.webui` | 0.5 vCPU | 0.5 GiB | 32 |
| [embedding-gemma-text-8](embedding-gemma-text-8/README.md) | `org.xelab.embedding-gemma.text-8` | 1 vCPU | 4 GiB | 8 |
| [embedding-gemma-text-1](embedding-gemma-text-1/README.md) | `org.xelab.embedding-gemma.text-1` | 2 vCPU | 4 GiB | 1 |
| [embedding-gemma-multimodal-1](embedding-gemma-multimodal-1/README.md) | `org.xelab.embedding-gemma.multimodal-1` | 1 vCPU | 6 GiB | 1 |
| [knowledge-base](knowledge-base/README.md) | `org.xelab.knowledge-base` | 2 vCPU | 4 GiB | 8 |

三个 embedding 数值为用户指定预设。WebUI 的0.5C/0.5GiB和知识库的2C/4GiB、20GiB数据卷为可调整的初始建议，不是测量值。默认均单副本，安装后 disabled，模板 UUID/来源域名需替换。

三个 embedding 同时运行共4 vCPU/14GiB；再加内核至少4vCPU/16GiB，尚未包含 WebUI、知识库、OS/数据库、日志和升级余量。全部实例均从插件池扣除，不借内核预留。

[机器目录](catalog.json) · [EmbeddingGemma模型与锁定](embedding-gemma-model.md) · [核心Skill层](../skills/README.md)。

当前共7个普通OCI预设和1个微预设；Micro Plugin Runtime是其中的普通OCI预设；依赖它的[微插件](../architecture/micro-plugins.md)为独立核心类型，不为每个脚本建OCI。MPR资源同样计入插件池；1C/1GiB为未实测建议，注册脚本不自动授予执行或运行时使用权。

网页搜索只转发批准的服务商API，优先级和fallback按每微部署HttpBinding配置；[对象存储](object-storage/README.md)提供按用户隔离的持久文件。机器目录的presets仅为OCI，micro_presets单列微制品，不能把网页搜索当作额外容器安装。
