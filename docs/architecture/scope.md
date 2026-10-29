# 范围、边界与决策

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 固定前提

- 单台 Linux 主机，cgroups v2，amd64 或 arm64；CPU 插件，不支持 GPU。
- 内核最低预留 4000 millicpu / 17179869184 bytes；宿主、运行时、独立 PostgreSQL 等另行预留。
- 本机调度；同机多实例不构成宿主高可用，不承诺无中断升级或跨节点故障转移。
- 内核使用 Rust；普通插件可用 Rust/C++ 或任何能实现插件RPC的语言，使用独立OCI制品及部署。
- 核心另行管理MicroPlugin、不可变MicroPluginVersion及MicroDeployment；微插件无需自己的OCI镜像或容器，由Micro Plugin Runtime承载。
- 预设插件包括WebUI、三种EmbeddingGemma 2、可选向量的知识库及Micro Plugin Runtime，见 [目录](../preset-plugins/README.md)。
- 业务Skill由内核验证和转发，固定普通安装来源，或固定微版本、微制品摘要及微部署；不按名字解析“最新版”。
- WebUI 为独立 OCI 插件；内核保留认证、授权、管理 API、本机 CLI 与最小可信登录页面。
- Guest 为已认证账户角色；匿名访问如启用则使用 Anonymous 主体。
- 权限组扁平、版本不可变；账户为单父级树，源凭证链为无环有向关系。

## 已冻结的协议选择

| 项目 | 选择 | 原因与约束 |
|---|---|---|
| HTTP | OpenAPI 3.1.1，JSON Schema 2020-12 | HTTP 结构唯一来源为 contracts/openapi.json |
| 普通插件调用 | gRPC / proto3 | 在受控网络通道上传递内核签发的委托 |
| 微插件调用 | 核心Skill路由 → MicroRuntimeService → 受限JS | json-stdio-v1单次JSON输入/输出；不开放用户到运行时的直连执行 |
| 普通制品 | OCI 镜像，固定 digest，Ed25519 签名 | tag 不能作为部署时可变输入 |
| 微制品 | 核心登记的不可变MicroManifest、源码与package/source摘要 | 单独核验所有者和发布来源，不把动态源码当作受MPR OCI签名覆盖 |
| 元数据 | PostgreSQL | 事务、行锁、outbox 支撑对账及租约 |
| 事件/任务 | PostgreSQL 持久表与 outbox | 首版不额外引入外部消息中间件 |
| 数据卷 | XFS project quota 或具备等效硬配额的后端 | 启动自检不通过则不能声明容量隔离已就绪 |
| API Key | 随机不透明凭证 + 服务端可撤销记录 | 不依赖长期 JWT 中的角色进行授权 |

## 扩展边界

新增现有契约的实现不得修改内核。新增能力契约需发布 Schema、契约摘要及测试；新增外部协议适配器属于新的受审模块，不能假设任意业务协议可自动转换。

微插件是本版明确增加的核心对象和执行目标，不是普通Plugin/Installation的别名。MPR本身仍为受管OCI预设`org.xelab.micro-plugin-runtime@1.1.0`；一个实例可承载多个微插件，节点只为MPR父部署预留资源。边界与状态见[微插件架构](micro-plugins.md)。

首版微插件只支持受限JavaScript纯计算，`side_effect=none`，无文件、网络、secret或HostService绑定。它不允许任意Python/Node宿主裸进程、shell命令或嵌套容器；其他引擎需新定义执行与隔离契约。

本版仅冻结文本 chat/completions、embedding 的明确子集，详见 [兼容协议](../communication/ai-profile.md)。其他 AI 模态通过版本化能力新增，不冒充完整兼容。

多节点、权限组嵌套、任意脚本工作流、推送至任意 URL 的 webhook、GPU、计费结算均未定义为本版公开接口，禁止实现时自行推断隐含契约。

## API转发与文件存储预设

网页搜索以controlled-http-v1微预设存在，只经核心固定API模板代理；默认纯微插件仍无网络。对象存储为普通OCI插件，以可信subject命名空间持久存文件，和核心临时Blob分工，见[预设目录](../preset-plugins/README.md)。
