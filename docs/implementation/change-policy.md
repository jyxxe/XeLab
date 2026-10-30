# 契约优先级、变更与文档维护

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 规范来源

HTTP 字段/路径/状态结构以 contracts/openapi.json 为唯一来源，生成 api/operations 与 api/models；RPC 消息和字段号以 .proto 为来源，方法行为以 rpc-semantics.json 及主题规则补充；进程内类型/方法以 internal.schema.json 与 internal-interfaces.json 为来源；持久化以 metadata.sql 和事务规则共同定义。

人类文档和机器结构冲突应视为规格缺陷并在实现前修复，不默认为代码或示例优先。接口示例不是额外字段声明，archive 不参与现行规范。

## 变更步骤

1. 先改对应机器契约，补充业务状态/授权/幂等/错误语义。
2. 运行 render_reference.py；修改 Proto/方法元数据后运行 render_rpc.py。
3. 更新测试样例、主题文档和原计划映射。
4. 运行 validate_docs.py --strict，再审查接口差异与范围扩张。
5. 记录兼容性：新增可选字段不得悄悄改变快照授权；新动作不能自动进入已冻结的旧权限上限。

## 版本规则

当前整套设计版本 2.8.0，管理 HTTP/Proto 仍为 v1。文档重构版本不等于 API 主版本。未实现时期可通过明确规格变更调整；一旦对外发布，删除/改类型/改变错误或副作用语义需新主版本或兼容迁移窗口。

新增功能如果需要新端点，必须同时提供请求/响应字段、认证、授权目标、前置版本、幂等、错误、取消及验收；禁止只写“后续扩展 API”而将其当已定义能力。

新增机器来源：preset-plugins/catalog.json及各预设manifest/defaults/contracts、skills中的逐业务定义。接口变更需同步目标Schema/签名摘要与Skill副本，并运行扩展严格检查；内部方法变更运行render_internal.py。

iframe消息以contracts/ui-bridge.schema.json为结构来源，ui-bridge与逐方法文档定义来源校验/白名单等运行语义。异步batch以OpenAPI、BatchEngine内部接口及SQL共同定义；新增动作不进入已有账户/Key冻结上限。

受控HTTP是显式新profile；旧pure微制品省略新增字段仍解释纯计算，不重写其源码或package_digest。MicroHttpBinding优先级可配置而代码模板不可原地改，出网记录按Call冻结；non_repeatable不得继承旧pure的自动重试。微预设在catalog.micro_presets单列，和OCI镜像制品验证分开。

2.7把微部署resources及network_enabled与不可变业务limits分开。创建请求省略设置时由核心显式填入250mCPU/256MiB/false；现有元数据迁移也按相同默认值补齐并失效旧Prepare，历史HTTP部署须手动确认联网。微清单、源码和包摘要不因补部署设置而重写；旧Runtime缺少cgroup资源隔离声明时拒绝新微执行，不降级成仅父cgroup/JS heap限制。

MPR1.2父默认4vCPU/4GiB，微资源设置在停用且排空后以If-Match及幂等键修改，更新generation/revision，不改候选Skill。新增HostService.AttachMicroWorker与Runtime内部组管理方法仅为监督器/核心受审适配器提供可信资源准入，不给微脚本添加宿主RPC或cgroup操作权限。

## 2.8扩展类型兼容规则

ExtensionPlugin为第三种独立核心类型，宿主可为普通、微或扩展，必须显式声明版本化extension_points；没有声明的旧清单保持不支持，不回填字段或重算既有包/签名。独立扩展来源也不由宿主签名自动覆盖。

新增data/skill模式、typed宿主、固定host_chain_digest、根ExtensionHostService和extension Skill目标。扩展配置、版本、宿主及limits变化均创建新部署/候选；现有工作流与batch保留原链。MPR1.3的扩展适配必须协商支持，不把扩展Data或handler桥静默加入旧运行时；资源、网络profile和调用者当前授权上限不得扩张。
