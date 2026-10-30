# 角色、Guest 与默认授权

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 模板与动作

角色为内置职责模板，绑定时给出 ResourceSelector；不是只读角色名称后直接放行。完整可注册动作见 [权限目录](permissions.md)，操作绑定见 [HTTP 目录](../api/README.md)。

| 角色 | 默认动作范围 | 不自动获得 |
|---|---|---|
| Owner | 所有当前注册的平台动作；受强制系统边界限制 | 内核预留降低、源链脱离、原始凭证读取后门 |
| Operator | plugin.read、plugin.enable、plugin.upgrade、operation.read/cancel、platform.read、plugin.logs.read | 任意镜像安装、插件扩权、secret 明文 |
| Member | capability.discover/invoke、本人 task/workflow/schedule/blob 操作 | 他人任务、插件部署、平台授权管理 |
| Auditor | audit.read、platform.read、plugin.read，均脱敏 | 普通业务输入输出、原始日志、secret |
| Guest | 明确共享的 capability.discover/invoke 和获准页面 | 任何全局管理、其他用户数据 |

所有 active 用户额外拥有自身的 session.read/revoke、credential.api-key.create/read/revoke/rotate、account.child.create/manage、group.create/read/update/archive，以及本人权限查看。自助只作用于本人及授权后代，不包括任意账户。

这是创建用户时一并建立的最小身份自助规则，包括 Guest 和子账户；选择部分业务权限不能使一个正常登录的子用户失去创建受限下级/Key 的基本能力。该规则由内核固定，不授予平台运维、他人数据或高于自身业务上限的派生授权。API Key 本身不自动补齐这组动作，仍须在其有效范围内明确拥有相应自助动作。

角色必须与 scope 一起生效。Member/Guest 没有指向具体能力的有效绑定时不能调用该能力。默认权限组只是模板，发布时不自动给所有用户绑定。

## Guest 与 Anonymous

Guest 是有独立身份的用户，同样能在树配额内创建子账户/密钥，后代不会自动获得 Member 权限。Anonymous 仅用于明确开放的静态入口；不创建账户或凭证，不持有任务和私有资源。

## Owner 边界

可以把全部管理动作委托给子账户，但子账户仍保留父级上限，不能创建脱离该链的新根。independent_owner 是宿主引导/恢复建立的根属性，不随角色复制。最后一名有效独立 Owner 不能被归档或禁用。

## 批次与UI默认范围

Member模板增加本人skill_batch的batch.create/read/cancel/resume；Skill/目标动作仍须明确授权。plugin.ui.open仅针对批准可见/共享插件，不因为Member就开放所有页面。Guest可按明确共享规则获得UI与batch范围，未共享默认拒绝。ui.preferences.read/update属于本人非敏感资料自助，派生Key仍须选入。Operator/Auditor不因管理/审计职责自动取得业务批次结果。

## 微插件默认授权

Member可按本人owned_by范围获得micro.create/read/update/publish/source.read/deploy/enable/archive/invoke；具体Skill.read/invoke和可用MPR部署上的micro.runtime.use仍须明确授予。Guest只可调用明确共享的微资源/Skill及获准父运行时。父子关系、相同角色和运行时管理权不隐含微代码/调用诊断共享；跨用户按实际核心grants检查。API Key仍与其固定上限求交，新动作不自动进入旧快照。
