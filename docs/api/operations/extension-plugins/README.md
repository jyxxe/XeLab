# extension-plugins 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/extension-plugins` | [列出可见扩展插件](listExtensionPlugins.md) |
| POST | `/api/platform/v1/extension-plugins` | [创建扩展插件身份](createExtensionPlugin.md) |
| GET | `/api/platform/v1/extension-plugins/{extension_plugin_id}` | [读取扩展插件身份](getExtensionPlugin.md) |
| PATCH | `/api/platform/v1/extension-plugins/{extension_plugin_id}` | [更新扩展显示资料](updateExtensionPlugin.md) |
| DELETE | `/api/platform/v1/extension-plugins/{extension_plugin_id}` | [归档扩展插件及其绑定](archiveExtensionPlugin.md) |
| GET | `/api/platform/v1/extension-plugins/{extension_plugin_id}/versions` | [列出不可变扩展版本](listExtensionVersions.md) |
| POST | `/api/platform/v1/extension-plugins/{extension_plugin_id}/versions` | [发布JSON贡献与扩展清单](publishExtensionVersion.md) |
| GET | `/api/platform/v1/extension-plugins/{extension_plugin_id}/versions/{extension_version_id}` | [读取精确扩展版本](getExtensionVersion.md) |
| GET | `/api/platform/v1/extension-plugins/{extension_plugin_id}/versions/{extension_version_id}/source` | [读取扩展贡献原始JSON](getExtensionSource.md) |
| GET | `/api/platform/v1/extension-deployments` | [列出扩展运行绑定](listExtensionDeployments.md) |
| POST | `/api/platform/v1/extension-deployments` | [固定扩展版本、宿主与扩展点](createExtensionDeployment.md) |
| GET | `/api/platform/v1/extension-deployments/{extension_deployment_id}` | [读取扩展固定宿主链与状态](getExtensionDeployment.md) |
| DELETE | `/api/platform/v1/extension-deployments/{extension_deployment_id}` | [归档扩展绑定](archiveExtensionDeployment.md) |
| PUT | `/api/platform/v1/extension-deployments/{extension_deployment_id}/enabled` | [启停扩展运行绑定](setExtensionDeploymentEnabled.md) |
