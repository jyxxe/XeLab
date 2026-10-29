# plugins 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/publishers` | [列出发布者信任根](listPublishers.md) |
| POST | `/api/platform/v1/publishers` | [信任发布者公钥](trustPublisher.md) |
| GET | `/api/platform/v1/publishers/{publisher_id}` | [读取发布者及版本](getPublisher.md) |
| DELETE | `/api/platform/v1/publishers/{publisher_id}` | [撤销发布者信任](revokePublisher.md) |
| GET | `/api/platform/v1/installations` | [列出已安装插件版本](listInstallations.md) |
| POST | `/api/platform/v1/installations` | [验证并安装插件制品](installPlugin.md) |
| GET | `/api/platform/v1/installations/{installation_id}` | [读取安装与清单](getInstallation.md) |
| DELETE | `/api/platform/v1/installations/{installation_id}` | [卸载插件程序版本](uninstallPlugin.md) |
| GET | `/api/platform/v1/deployments` | [列出部署](listDeployments.md) |
| POST | `/api/platform/v1/deployments` | [创建部署期望配置](createDeployment.md) |
| GET | `/api/platform/v1/deployments/{deployment_id}` | [读取部署及当前代次](getDeployment.md) |
| PUT | `/api/platform/v1/deployments/{deployment_id}` | [更新完整部署配置](updateDeployment.md) |
| DELETE | `/api/platform/v1/deployments/{deployment_id}` | [排空并归档部署](deleteDeployment.md) |
| PUT | `/api/platform/v1/deployments/{deployment_id}/enabled` | [只启用或禁用已批准部署](setDeploymentEnabled.md) |
| PUT | `/api/platform/v1/deployments/{deployment_id}/approval` | [批准插件代理与出口权限](approvePlugin.md) |
| GET | `/api/platform/v1/deployments/{deployment_id}/approval` | [读取插件实际审批范围](getPluginApproval.md) |
| POST | `/api/platform/v1/deployments/{deployment_id}/upgrade` | [升级到已安装版本](upgradeDeployment.md) |
| POST | `/api/platform/v1/deployments/{deployment_id}/rollback` | [程序回退或经核实的数据恢复](rollbackDeployment.md) |
| GET | `/api/platform/v1/instances` | [列出实际实例](listInstances.md) |
| GET | `/api/platform/v1/instances/{instance_id}` | [读取实例状态](getInstance.md) |
| GET | `/api/platform/v1/instances/{instance_id}/logs` | [读取脱敏日志](listInstanceLogs.md) |
| POST | `/api/platform/v1/dependency-plans` | [预览依赖和资源计划](resolveDependencies.md) |
