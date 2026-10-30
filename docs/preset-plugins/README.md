# 预设插件目录

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

每个插件占用一个独立子目录，包含说明、配置、接口/Skill、资源及验收。contracts是共享契约目录，不作为插件。通用开发规则仍在 [plugins](../plugins/README.md)，避免复制大段人工说明。

[Extension Plugin](../extensions/README.md)是新增的第三种插件类型，可依附普通、微或其他扩展插件的明确扩展点。本轮定义通用模型与结构样例，不把样例当作新OCI或微预设加入安装目录。

新增[12个普通插件与20个微插件规划](planned/README.md)，包含[37个逐能力接口](planned/contracts.md)。规划与下表已有预设分开编目，依赖、库及核心适配尚待实现；[普通插件](planned/ordinary.md)和[微插件](planned/micro.md)分别提供清晰的目录/能力/依赖/用途索引。

| 插件 | ID | CPU默认 | 内存默认 | 实例活动并发 |
|---|---|---|---|---|
| [web-search](web-search/README.md) | `org.xelab.web-search`（微预设） | 共享MPR | 受微限额约束 | 默认1、硬4 |
| [object-storage](object-storage/README.md) | `org.xelab.object-storage` | 1 vCPU | 1 GiB | 4 |
| [micro-plugin-runtime](micro-plugin-runtime/README.md) | `org.xelab.micro-plugin-runtime` | 4 vCPU | 4 GiB | 4 |
| [webui](webui/README.md) | `org.xelab.webui` | 0.5 vCPU | 0.5 GiB | 32 |
| [embedding-gemma-text-8](embedding-gemma-text-8/README.md) | `org.xelab.embedding-gemma.text-8` | 1 vCPU | 4 GiB | 8 |
| [embedding-gemma-text-1](embedding-gemma-text-1/README.md) | `org.xelab.embedding-gemma.text-1` | 2 vCPU | 4 GiB | 1 |
| [embedding-gemma-multimodal-1](embedding-gemma-multimodal-1/README.md) | `org.xelab.embedding-gemma.multimodal-1` | 1 vCPU | 6 GiB | 1 |
| [knowledge-base](knowledge-base/README.md) | `org.xelab.knowledge-base` | 2 vCPU | 4 GiB | 8 |

三个 embedding 数值为用户指定预设。WebUI 的0.5C/0.5GiB和知识库的2C/4GiB、20GiB数据卷为可调整的初始建议，不是测量值。默认均单副本，安装后 disabled，模板 UUID/来源域名需替换。

三个 embedding 同时运行共4 vCPU/14GiB；再加内核至少4vCPU/16GiB，尚未包含 WebUI、知识库、OS/数据库、日志和升级余量。全部实例均从插件池扣除，不借内核预留。

[机器目录](catalog.json) · [EmbeddingGemma模型与锁定](embedding-gemma-model.md) · [核心Skill层](../skills/README.md)。

当前共7个普通OCI预设和1个微预设；Micro Plugin Runtime是其中的普通OCI预设；依赖它的[微插件](../architecture/micro-plugins.md)为独立核心类型，不为每个脚本建OCI。MPR父容器默认4C/4GiB，资源同样计入插件池；每微部署默认250mCPU/256MiB、联网关闭，设置可手动修改。注册脚本不自动授予执行或运行时使用权。

启用默认MPR时，与内核至少4C/16GiB预留合计8vCPU/20GiB，仍需另加OS/数据库等宿主基础设施与其他插件。微部署资源是MPR父预算内的聚合硬限制，不重复向节点申请一份父预算，也不能借用内核预留。

网页搜索只转发批准的服务商API，优先级和fallback按每微部署HttpBinding配置；[对象存储](object-storage/README.md)提供按用户隔离的持久文件。机器目录的presets仅为OCI，micro_presets单列微制品，不能把网页搜索当作额外容器安装。
