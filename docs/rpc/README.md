# 插件与宿主 RPC

[文档总目录](../README.md) · [Protobuf 契约](../contracts/README.md) · [公共语义](../communication/grpc.md)

## CapabilityService

[机器协议源文件](../contracts/proto/xelab/v1/capability.proto)

- [CapabilityService.Invoke](CapabilityService.Invoke.md)
- [CapabilityService.InvokeStream](CapabilityService.InvokeStream.md)
- [CapabilityService.Cancel](CapabilityService.Cancel.md)
- [CapabilityService.QueryResult](CapabilityService.QueryResult.md)

## ExtensionHostService

[机器协议源文件](../contracts/proto/xelab/v1/extension.proto)

- [ExtensionHostService.Prepare](ExtensionHostService.Prepare.md)
- [ExtensionHostService.Invoke](ExtensionHostService.Invoke.md)
- [ExtensionHostService.Release](ExtensionHostService.Release.md)

## HostService

[机器协议源文件](../contracts/proto/xelab/v1/host.proto)

- [HostService.AttachMicroWorker](HostService.AttachMicroWorker.md)
- [HostService.ProxyHttp](HostService.ProxyHttp.md)
- [HostService.ListSkillCandidates](HostService.ListSkillCandidates.md)
- [HostService.ResolveSkill](HostService.ResolveSkill.md)
- [HostService.CheckSkillAccess](HostService.CheckSkillAccess.md)
- [HostService.InvokeSkill](HostService.InvokeSkill.md)
- [HostService.InvokeSkillStream](HostService.InvokeSkillStream.md)
- [HostService.Resolve](HostService.Resolve.md)
- [HostService.Invoke](HostService.Invoke.md)
- [HostService.InvokeStream](HostService.InvokeStream.md)
- [HostService.PublishEvent](HostService.PublishEvent.md)
- [HostService.Subscribe](HostService.Subscribe.md)
- [HostService.AckEvent](HostService.AckEvent.md)
- [HostService.ReadBlob](HostService.ReadBlob.md)
- [HostService.WriteBlob](HostService.WriteBlob.md)
- [HostService.LeaseSecret](HostService.LeaseSecret.md)

## PluginLifecycle

[机器协议源文件](../contracts/proto/xelab/v1/lifecycle.proto)

- [PluginLifecycle.Describe](PluginLifecycle.Describe.md)
- [PluginLifecycle.Initialize](PluginLifecycle.Initialize.md)
- [PluginLifecycle.GetHealth](PluginLifecycle.GetHealth.md)
- [PluginLifecycle.Drain](PluginLifecycle.Drain.md)
- [PluginLifecycle.Shutdown](PluginLifecycle.Shutdown.md)

## MicroRuntimeService

[机器协议源文件](../contracts/proto/xelab/v1/micro.proto)

- [MicroRuntimeService.Prepare](MicroRuntimeService.Prepare.md)
- [MicroRuntimeService.Invoke](MicroRuntimeService.Invoke.md)
- [MicroRuntimeService.Release](MicroRuntimeService.Release.md)

## DataMigration

[机器协议源文件](../contracts/proto/xelab/v1/migration.proto)

- [DataMigration.Plan](DataMigration.Plan.md)
- [DataMigration.Apply](DataMigration.Apply.md)
- [DataMigration.Verify](DataMigration.Verify.md)

## TaskWorker

[机器协议源文件](../contracts/proto/xelab/v1/task.proto)

- [TaskWorker.Execute](TaskWorker.Execute.md)
- [TaskWorker.Cancel](TaskWorker.Cancel.md)
- [TaskWorker.Query](TaskWorker.Query.md)

