# 领域对象与不变量

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 身份、版本与实例分离

| 实体 | 标识与版本 | 关系与不变量 |
|---|---|---|
| Plugin | plugin_id | 稳定产品标识，不携带运行状态 |
| Installation | UUID + plugin_id/version/digest | 同一插件版本不可对应不同 digest；已安装不表示可调用 |
| Deployment | UUID + revision | 引用一个安装版本，保存 enabled/replicas/config/limits |
| Instance | UUID + generation | 属于部署代次；容器 ID 不可替代实例 ID |
| MicroPlugin | UUID + owner/name/status | 独立核心资源；不是plugin_id或普通Installation，默认无跨账户共享 |
| MicroPluginVersion | UUID + manifest/package_digest/source_digest | 内容不可变，固定runtime_installation_id；源码改变发布新版本 |
| MicroDeployment | UUID + owner/revision/generation | 固定版本/limits/父绑定；resources及network_enabled只在disabled且完全排空时变更，自身不创建OCI |
| MicroResourcePool / Group | 核心UUID + 父实例/运行代次 | 真实cgroupv2工作池及微部署聚合组；资源从父池细分，不重复预留节点预算 |
| MicroWorkerSlot | execution_id + 当前Call | 不透明准入句柄固定实例/微部署/代次/Call；worker真实入组后才运行代码 |
| ExtensionPlugin / Version | 核心UUID + manifest/package摘要 | 独立扩展身份和来源，不由宿主签名自动覆盖；版本内容不可变 |
| ExtensionDeployment | UUID + 固定宿主/版本/config/limits | 宿主ordinary/micro/extension恰一项，初始disabled；变更固定项须新部署 |
| ExtensionPoint | 宿主版本内name/version/schema摘要 | 宿主显式接纳契约，可限制模式、输入输出及副作用；不靠同名猜兼容 |
| CapabilityContract | name/version/schema_digest | 语义、Schema、错误、副作用类型不可变 |
| Skill | UUID + name/version/definition_digest | 固定普通安装及接口，或固定微版本/制品/部署；注册不自动授权 |
| Binding | UUID | 固定提供者与范围；不是授权凭证 |
| Volume | UUID + revision | 卷生命周期独立，单写卷同时最多一个有效写入 fence |
| Task / Step / Attempt | 各自 UUID | 重试新增尝试，原结果不覆盖；未知不等于失败 |
| Account | UUID + parent_id | 父级关系归档后保留，不允许自助改为根 |
| Credential | UUID + source | 校验摘要不出接口，源链撤销约束不可移除 |

完整字段定义见 [模型目录](../api/models/README.md)，持久化约束见 [数据模型](../operations/persistence.md)。

## 全局不变量

1. 任何可用路由必须指向已批准、当前代次、逐能力 Ready 的实例。
2. 当前授权不能高于任意祖先、源凭证、固定委托上限或资源策略。
3. 所有活跃实例预留总和不超过插件池；新旧并行版本同时记账。
4. 容器启动成功和数据库写入成功不是原子事件；必须通过幂等外部操作和对账收敛。
5. 任何客户端不可直接设置 observed_generation、runtime_id、independent_owner、authz_revision。
6. 数据删除与程序卸载分为不同接口及权限；返回 202 不表示删除已完成。
7. 父子关系授予管理边界，不能自动解释成双方业务数据互读。
8. MicroPluginVersion的runtime_installation_id必须与其MicroDeployment所绑定普通运行时部署的实际安装相同；MPR更换安装版本需显式发布/迁移微版本和Skill。
9. 微Skill的micro_version_id、package_digest、micro_deployment_id与来源安装共同固定；输入JSON不能改写这些目标。
10. 微执行资源是MPR父部署预算中的受限使用，不是另一份节点预留；活动槽、执行内存及准备缓存均计入父部署。默认每微部署CPU250m/内存256MiB由该部署全部worker共享，MPR默认4000m/4GiB、活动硬槽4。
11. 微部署资源/联网设置只能在enabled=false、phase=disabled且队列/worker/Prepare/资源租约均已排空时修改；revision/generation递增作废旧准备句柄，候选Skill的Schema/timeout/target不变。

UUID 为服务端生成的 RFC 4122 UUID v4；排序使用 created_at/id。业务名称、插件名称与模型别名另有格式，不混用 UUID。

## 微版本与候选Skill

发布微版本先校验当前授权所有者、MicroManifest、包/源码摘要、业务Schema及绑定的MPR安装；版本本身不含尚未绑定部署的候选定义。创建MicroDeployment时固定父运行时部署及limits，再生成deployment.candidate_skill，初始enabled=false、phase=disabled；生成不等于注册或启用。候选业务输入/输出来自不可变MicroManifest，不来自运行时公共控制封装。

候选名固定为org.xelab.micro-plugin-runtime.micro.<micro_deployment_uuid>.run，版本为MicroManifest.version；定义同时固定该微部署的超时上限。更换limits或父运行时绑定需新建微部署与候选，不能原地改写旧Skill。

`resources`与`network_enabled`是微部署设置，不是可变脚本输入。PUT settings按ETag及幂等规则完整替换，只允许排空后的disabled微部署；pure profile不能开启联网。联网默认false，controlled-http-v1在关闭时于worker/HTTP sequence之前拒绝执行，已有绑定/出口审批不替代开关。

每活跃微部署及其generation只有一个实际微资源组，多个Call独立准入租约映射同组；最后真实worker与租约回收后才释放该组CPU/内存预留。句柄不携带宿主cgroup路径，也不授予MPR写宿主cgroup的权限。

registerSkill的installation_id必须等于微版本runtime_installation_id。微制品的核心发布记录与摘要是独立来源，不能宣称它被原MPR OCI镜像签名覆盖。微部署停用和MicroPlugin归档撤销新调用，已发布版本、历史Call及执行证据继续保留。详见[微插件架构](micro-plugins.md)。

## 扩展对象与固定宿主链

ExtensionPlugin、不可变ExtensionPluginVersion与ExtensionDeployment具有独立核心身份和来源记录，不是普通Installation或MicroPlugin的别名。扩展部署固定版本、宿主、配置和业务limits，初始disabled；更换任一固定项需要新扩展部署，候选Skill不允许原地漂移。

宿主选择ordinary/micro/extension恰一项，要求精确版本声明对应extension_point。扩展本身也可发布自己的扩展点，由此形成链；普通强依赖、micro→MPR、扩展→宿主和skill处理器目标依赖合并校验为DAG，完整依赖链最多16层，拒绝自引用及跨类型环。

扩展候选固定extension版本、部署、package_digest和host_chain_digest，installation_id解析为实际根OCI安装。宿主基础Ready不反向依赖其加载扩展；宿主失效使后代扩展waiting_host，不把可选扩展失效伪装为整个宿主基础服务故障。
