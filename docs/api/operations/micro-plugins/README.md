# micro-plugins 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/micro-plugins` | [列出可见微插件](listMicroPlugins.md) |
| POST | `/api/platform/v1/micro-plugins` | [创建微插件身份](createMicroPlugin.md) |
| GET | `/api/platform/v1/micro-plugins/{micro_plugin_id}` | [读取微插件元数据](getMicroPlugin.md) |
| PATCH | `/api/platform/v1/micro-plugins/{micro_plugin_id}` | [更新微插件显示资料](updateMicroPlugin.md) |
| DELETE | `/api/platform/v1/micro-plugins/{micro_plugin_id}` | [归档微插件及撤销新执行](archiveMicroPlugin.md) |
| GET | `/api/platform/v1/micro-plugins/{micro_plugin_id}/versions` | [列出不可变微版本](listMicroVersions.md) |
| POST | `/api/platform/v1/micro-plugins/{micro_plugin_id}/versions` | [发布固定微脚本制品](publishMicroVersion.md) |
| GET | `/api/platform/v1/micro-plugins/{micro_plugin_id}/versions/{micro_version_id}` | [读取微版本清单与来源](getMicroVersion.md) |
| GET | `/api/platform/v1/micro-plugins/{micro_plugin_id}/versions/{micro_version_id}/source` | [读取精确微版本源码](getMicroSource.md) |
| GET | `/api/platform/v1/micro-deployments` | [列出微插件运行绑定](listMicroDeployments.md) |
| POST | `/api/platform/v1/micro-deployments` | [绑定微版本和MPR父部署](createMicroDeployment.md) |
| GET | `/api/platform/v1/micro-deployments/{micro_deployment_id}` | [读取微部署状态和候选Skill](getMicroDeployment.md) |
| DELETE | `/api/platform/v1/micro-deployments/{micro_deployment_id}` | [归档微插件运行绑定](archiveMicroDeployment.md) |
| PUT | `/api/platform/v1/micro-deployments/{micro_deployment_id}/enabled` | [启停微插件运行绑定](setMicroDeploymentEnabled.md) |
| GET | `/api/platform/v1/micro-calls/{call_id}/diagnostics` | [读取本人或获准微调用诊断](getMicroCallDiagnostics.md) |
