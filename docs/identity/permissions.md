# 权限动作与授权目标目录

[身份权限目录](README.md) · [机器目录](../contracts/permissions.json)

> 规格版本：2.8.0；状态：待实现的规范。

每个操作的 x-authorization 明确实际目标来源；动作不能仅按名称前缀猜测对象类型。同一动作可用于多类明确注册的资源，但具体请求仍使用对应实际类型。

| 动作 | 可接受资源类型 | 自助 | HTTP 操作或语义 |
|---|---|---|---|
| `account.child.create` | account | 仅本人/授权后代 | createChildAccount |
| `account.child.manage` | account | 仅本人/授权后代 | updateInheritance |
| `account.manage` | account | 无默认自助 | updateAccount, archiveAccount |
| `account.permissions.read` | account | 仅本人/授权后代 | getEffectivePermissions, listGrants, getGrant |
| `account.profile.update` | account | 仅本人/授权后代 | 仅本人显示名称等非安全资料变更 |
| `account.read` | account | 无默认自助 | listAccounts, getAccount |
| `audit.read` | audit | 无默认自助 | listAuditEvents |
| `backup.create` | backup | 无默认自助 | createBackup |
| `backup.read` | backup | 无默认自助 | listBackups, getBackup |
| `backup.restore` | backup | 无默认自助 | restoreBackup |
| `blob.create` | blob | 无默认自助 | uploadBlob |
| `blob.delete` | blob | 无默认自助 | deleteBlob |
| `blob.read` | blob | 无默认自助 | getBlob, downloadBlob |
| `capability.call.cancel` | call | 仅本人/授权后代 | cancelCall |
| `capability.call.read` | call | 仅本人/授权后代 | getCall |
| `capability.discover` | capability, model | 无默认自助 | listCapabilities, getCapabilityContract, listModelAliases |
| `capability.invoke` | capability, model | 无默认自助 | invokeCapability, streamCapability, createChatCompletion, createEmbedding |
| `client.create` | client | 无默认自助 | createClient |
| `client.disable` | client | 无默认自助 | disableClient |
| `client.read` | client | 无默认自助 | listClients, getClient |
| `client.update` | client | 无默认自助 | updateClient |
| `credential.api-key.create` | account | 仅本人/授权后代 | createApiKey |
| `credential.api-key.read` | account, credential | 仅本人/授权后代 | listApiKeys, getApiKey |
| `credential.api-key.revoke` | credential | 仅本人/授权后代 | revokeApiKey |
| `credential.api-key.rotate` | credential | 仅本人/授权后代 | rotateApiKey |
| `event.publish` | event | 无默认自助 | publishEvent |
| `event.subscribe` | event | 无默认自助 | listSubscriptions, createSubscription, getSubscription, deleteSubscription, streamDeliveries, ackDelivery |
| `grant.create` | account | 无默认自助 | createGrant |
| `grant.revoke` | account | 无默认自助 | revokeGrant |
| `group.archive` | group | 仅本人/授权后代 | archiveGroup |
| `group.create` | group | 仅本人/授权后代 | createGroup |
| `group.publish` | group | 无默认自助 | 发布公共权限组，条件动作 |
| `group.read` | group | 仅本人/授权后代 | listGroups, getGroup, getGroupVersion |
| `group.update` | group | 仅本人/授权后代 | publishGroupVersion |
| `model.manage` | model | 无默认自助 | createAliasMapping, updateAliasMapping, deleteAliasMapping |
| `model.read` | model | 无默认自助 | listAliasMappings, getAliasMapping |
| `operation.cancel` | operation | 无默认自助 | cancelOperation |
| `operation.read` | operation | 无默认自助 | getOperation |
| `platform.configure` | platform | 无默认自助 | updateNodeConfig, setDefaultUi |
| `platform.read` | platform | 无默认自助 | getNode, getMetrics, getDefaultUi |
| `plugin.configure` | deployment | 无默认自助 | updateDeployment |
| `plugin.delete` | deployment | 无默认自助 | deleteDeployment |
| `plugin.dependencies.read` | deployment | 无默认自助 | resolveDependencies |
| `plugin.deploy` | deployment | 无默认自助 | createDeployment |
| `plugin.enable` | deployment | 无默认自助 | setDeploymentEnabled |
| `plugin.http.invoke` | plugin | 无默认自助 | 声明式 HTTP 路由默认业务调用动作；插件可另注册自己命名空间动作 |
| `plugin.install` | plugin | 无默认自助 | installPlugin |
| `plugin.logs.read` | instance | 无默认自助 | listInstanceLogs |
| `plugin.permission.grant` | deployment | 无默认自助 | approvePlugin |
| `plugin.read` | plugin, deployment, instance | 无默认自助 | listInstallations, getInstallation, listDeployments, getDeployment, getPluginApproval, listInstances, getInstance |
| `plugin.rollback` | deployment | 无默认自助 | rollbackDeployment |
| `plugin.uninstall` | plugin | 无默认自助 | uninstallPlugin |
| `plugin.upgrade` | deployment | 无默认自助 | upgradeDeployment |
| `publisher.read` | publisher | 无默认自助 | listPublishers, getPublisher |
| `publisher.revoke` | publisher | 无默认自助 | revokePublisher |
| `publisher.trust` | publisher | 无默认自助 | trustPublisher |
| `quota.manage` | account | 无默认自助 | updateQuota |
| `quota.read` | account | 无默认自助 | getQuota |
| `schedule.create` | schedule | 无默认自助 | createSchedule |
| `schedule.delete` | schedule | 无默认自助 | deleteSchedule |
| `schedule.read` | schedule | 无默认自助 | listSchedules, getSchedule |
| `schedule.update` | schedule | 无默认自助 | updateSchedule |
| `secret.bind` | secret | 无默认自助 | bindSecret, listSecretBindings, getSecretBinding, revokeSecretBinding |
| `secret.create` | secret | 无默认自助 | createSecret |
| `secret.metadata.read` | secret | 无默认自助 | listSecrets, getSecretMetadata |
| `secret.revoke` | secret | 无默认自助 | revokeSecret |
| `secret.rotate` | secret | 无默认自助 | rotateSecret |
| `secret.use` | secret | 无默认自助 | 仅批准的工作负载绑定内部使用，不是用户明文读取 |
| `session.read` | account | 仅本人/授权后代 | listSessions |
| `session.revoke` | credential | 仅本人/授权后代 | revokeSession |
| `task.cancel` | task | 无默认自助 | cancelTask |
| `task.create` | task | 无默认自助 | createTask |
| `task.read` | task | 无默认自助 | listTasks, getTask, listTaskSteps, listAttempts |
| `task.resolve` | task | 无默认自助 | resolveUnknownStep |
| `task.retry` | task | 无默认自助 | retryTask |
| `volume.create` | volume | 无默认自助 | createVolume |
| `volume.delete` | volume | 无默认自助 | deleteVolume |
| `volume.read` | volume | 无默认自助 | listVolumes, getVolume |
| `volume.resize` | volume | 无默认自助 | resizeVolume |
| `workflow.archive` | workflow | 无默认自助 | archiveWorkflow |
| `workflow.create` | workflow | 无默认自助 | createWorkflow |
| `workflow.read` | workflow | 无默认自助 | listWorkflows, getWorkflow |

## Skill动作

| 动作 | 可接受资源类型 | 自助 | HTTP操作 |
|---|---|---|---|
| `skill.archive` | skill | 无默认自助 | archiveSkill |
| `skill.enable` | skill | 无默认自助 | setSkillEnabled |
| `skill.invoke` | skill | 无默认自助 | invokeSkill, streamSkill |
| `skill.read` | skill | 无默认自助 | listSkills, getSkill |
| `skill.register` | skill | 无默认自助 | registerSkill |

## 字段与条件动作

updateAccount 仅改本人 display_name 时允许 account.profile.update；改 status 必须 account.manage 或合法父级管理权限。updateDeployment 如改变 enabled 还需 plugin.enable；只启停可使用 setDeploymentEnabled。公共组创建/发布须额外 group.publish。secret 绑定同时验证 secret.bind 与目标部署管理/审批权。

capability.invoke/discover 对通用能力匹配 capability 名称，对 AI 兼容入口匹配已解析的 model alias UUID；两种范围不混用。capability.call.read/cancel 匹配具体 Call UUID 和 owner。安装/卸载的 plugin 类型匹配稳定 plugin_id，不把任意安装 UUID 当插件名称。

自助不是无条件允许：先生成本人/获准后代范围的基础授权，再与 API Key、父级、客户端、执行委托和系统条件求交。Key 仅选 capability.invoke 时不能借自助规则额外操作凭证。

快照冻结当时已注册动作及规则选择器，不复制 Owner 的未来动作通配符。资源选择器仍按其明确的固定规则匹配未来资源，例如显式 all 匹配同类型新对象；这不同于自动增加新动作。

## 批次与界面动作

| 动作 | 资源 | 操作与边界 |
|---|---|---|
| batch.create | skill_batch | createSkillBatch，另需固定Skill及目标动作 |
| batch.read | skill_batch | 列表/状态/项目/结果；结果另检查当前业务读取权 |
| batch.cancel / batch.resume | skill_batch | 明确范围取消；resume额外限制原owner显式重授权 |
| batch.resolve | skill_batch | 依据证据核实unknown，独立授予；不自动重放 |
| plugin.ui.open | plugin | UI发现/打开，不隐含管理或业务调用权 |
| ui.preferences.read/update | account | 仅本人非敏感偏好；仍受API Key上限约束 |

## 微插件动作

| 动作 | 资源类型 | 条件 |
|---|---|---|
| micro.create/read/update/archive | micro_plugin | 管理逻辑身份，默认本人scope，跨用户需明确授权 |
| micro.publish/source.read | micro_plugin | 发布不可变版本/读取源码；publish另需blob.read |
| micro.deploy/enable | micro_plugin | 管理固定运行绑定；deploy还允许停用排空后配置CPU/总内存/联网，创建/配置/启用另需父运行时使用权 |
| micro.invoke | micro_plugin | 与skill.invoke及micro.runtime.use求交；Key可按具体微插件UUID缩限 |
| micro.runtime.use | deployment | 使用批准的普通MPR部署，不授予普通部署启停/升级权 |
| micro.diagnostics.read | micro_plugin | 还需实际call上的capability.call.read，程序所有者不自动读其他调用者日志 |

完整HTTP绑定见[微插件接口](../api/operations/micro-plugins/README.md)。普通plugin.*动作不根据前缀自动覆盖micro.*；来源安装信任与微脚本owner/制品批准分别核实。

## 受控微HTTP

micro.http.read/bind作用于真实micro_plugin UUID；bind操作还核对所属MicroDeployment和必要secret.bind及父出口审批。micro.http.request仅允许当前micro Call使用已批准模板/绑定，与原skill.invoke、micro.invoke、micro.runtime.use和源链求交。Secret使用由核心代理的专用绑定授权，不向终端或MPR发原值/LeaseSecret能力；Member默认纯微权限不包含网络扩权。

network_enabled默认false。把它手动设为true只表达联网意图，不授予micro.http.request，也不能给pure-json-v1制品新增HTTP能力；当前开关、原部署generation及所有授权均在HTTP Invoke和每次代理请求检查。关闭时拒绝准入，不作为服务商失败执行fallback。

对象存储复用6个Skill与明确capability.invoke，插件内默认只访问当前subject命名空间；Key使用所属账户空间、子账户单独空间，不因父子/运维角色自动共享文件。

## 扩展插件权限

| 动作 | 资源类型 | 用途与条件 |
|---|---|---|
| extension.read | extension_plugin | 读取获准扩展身份、版本与部署；集合按当前scope过滤 |
| extension.create | extension_plugin | 创建独立扩展身份，owner来自可信上下文 |
| extension.update | extension_plugin | 修改身份显示资料，不改不可变版本或固定部署 |
| extension.archive | extension_plugin | 归档逻辑身份并撤新路由，保留历史及执行证据 |
| extension.publish | extension_plugin | 发布固定data/skill制品，单独核验内容及来源 |
| extension.source.read | extension_plugin | 读取精确版本的贡献/制品内容，不隐含其他账户读取权 |
| extension.deploy | extension_plugin | 创建或归档固定宿主/配置绑定，另查对应宿主接纳权 |
| extension.enable | extension_plugin | 启停扩展部署；不自动启用宿主、handler或候选Skill |
| extension.invoke | extension_plugin | 执行固定leaf扩展Skill，与skill.invoke及当前链权限求交 |
| extension.host.use | deployment / micro_plugin / extension_plugin | 使用明确typed宿主扩展点；不授予宿主其他业务、管理、Secret或网络权 |

ExtensionPlugin为独立资源，身份/发布/部署管理不沿用普通plugin.*或micro.*隐式授权。leaf业务调用要求extension.invoke，逐层宿主都需extension.host.use：ordinary作用于deployment，micro作用于真实micro_plugin，extension作用于真实extension_plugin。

普通根核验当前安装及扩展点审批，不额外索取无关capability.invoke；微根另查micro.invoke/runtime.use。祖先扩展只有实际执行其业务处理器时才查invoke/Skill，固定handler仍查原skill.invoke及目标动作。Skill注册、业务调用和宿主接纳为分开批准的环节。

对象归属或知道宿主UUID不表示可扩展、读取数据或使用其Secret。Guest、子账户、权限组和派生Key仍按完整来源链及当前上限求交；父级收权、宿主关闭或处理器授权失效均阻止后续调用。逐动作机器目录以[扩展接口](../extensions/README.md)和permissions.json为准。
