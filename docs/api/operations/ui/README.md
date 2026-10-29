# ui 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/ui/contributions` | [发现可用插件窗口与组件](listUiContributions.md) |
| GET | `/api/platform/v1/ui/contributions/{deployment_id}/{contribution_id}` | [核对固定UI贡献及桥接绑定](getUiContribution.md) |
| GET | `/api/platform/v1/me/ui-preferences` | [读取本人主题与桌面偏好](getMyUiPreferences.md) |
| PUT | `/api/platform/v1/me/ui-preferences` | [更新本人主题与桌面偏好](setMyUiPreferences.md) |
