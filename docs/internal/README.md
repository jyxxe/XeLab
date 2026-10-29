# 内核进程内接口

[文档总目录](../README.md) · [结构契约](../contracts/internal.schema.json) · [方法清单](../contracts/internal-interfaces.json)

HTTP handler 复用对应 operationId 的结构；下列为稳定模块协作边界，私有辅助函数不纳入稳定接口。

- [Identity](Identity.md)：2个方法
- [Authorizer](Authorizer.md)：2个方法
- [Delegator](Delegator.md)：1个方法
- [Quota](Quota.md)：2个方法
- [Admission](Admission.md)：2个方法
- [Runtime](Runtime.md)：3个方法
- [Registry](Registry.md)：3个方法
- [DependencyResolver](DependencyResolver.md)：1个方法
- [Lifecycle](Lifecycle.md)：1个方法
- [TaskEngine](TaskEngine.md)：2个方法
- [EventStore](EventStore.md)：3个方法
- [SecretBroker](SecretBroker.md)：1个方法
- [Audit](Audit.md)：1个方法
- [SkillRouter](SkillRouter.md)：2个方法
- [BatchEngine](BatchEngine.md)：4个方法
- [MicroRegistry](MicroRegistry.md)：1个方法
- [MicroController](MicroController.md)：1个方法
- [MicroExecutor](MicroExecutor.md)：1个方法
- [OutboundProxy](OutboundProxy.md)：1个方法
