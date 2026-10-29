# 领域对象与不变量

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 身份、版本与实例分离

| 实体 | 标识与版本 | 关系与不变量 |
|---|---|---|
| Plugin | plugin_id | 稳定产品标识，不携带运行状态 |
| Installation | UUID + plugin_id/version/digest | 同一插件版本不可对应不同 digest；已安装不表示可调用 |
| Deployment | UUID + revision | 引用一个安装版本，保存 enabled/replicas/config/limits |
| Instance | UUID + generation | 属于部署代次；容器 ID 不可替代实例 ID |
| MicroPlugin | UUID + owner/name/status | 独立核心资源；不是plugin_id或普通Installation，默认无跨账户共享 |
| MicroPluginVersion | UUID + manifest/package_digest/source_digest | 内容不可变，固定runtime_installation_id；源码改变发布新版本 |
| MicroDeployment | UUID + owner/revision | 固定micro_version_id及runtime_deployment_id，保存enabled/limits/phase/candidate_skill；limits及父绑定固定，自身不创建OCI |
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
10. 微执行资源是MPR父部署预算中的受限使用，不是另一份节点预留；活动槽、执行内存及准备缓存均计入父部署。

UUID 为服务端生成的 RFC 4122 UUID v4；排序使用 created_at/id。业务名称、插件名称与模型别名另有格式，不混用 UUID。

## 微版本与候选Skill

发布微版本先校验当前授权所有者、MicroManifest、包/源码摘要、业务Schema及绑定的MPR安装；版本本身不含尚未绑定部署的候选定义。创建MicroDeployment时固定父运行时部署及limits，再生成deployment.candidate_skill，初始enabled=false、phase=disabled；生成不等于注册或启用。候选业务输入/输出来自不可变MicroManifest，不来自运行时公共控制封装。

候选名固定为org.xelab.micro-plugin-runtime.micro.<micro_deployment_uuid>.run，版本为MicroManifest.version；定义同时固定该微部署的超时上限。更换limits或父运行时绑定需新建微部署与候选，不能原地改写旧Skill。

registerSkill的installation_id必须等于微版本runtime_installation_id。微制品的核心发布记录与摘要是独立来源，不能宣称它被原MPR OCI镜像签名覆盖。微部署停用和MicroPlugin归档撤销新调用，已发布版本、历史Call及执行证据继续保留。详见[微插件架构](micro-plugins.md)。
