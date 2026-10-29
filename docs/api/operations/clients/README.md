# clients 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/` | [跳转默认界面](redirectDefaultUi.md) |
| GET | `/api/platform/v1/clients` | [列出 WebUI 客户端](listClients.md) |
| POST | `/api/platform/v1/clients` | [注册 WebUI 公共客户端](createClient.md) |
| GET | `/api/platform/v1/clients/{client_id}` | [读取客户端配置](getClient.md) |
| PUT | `/api/platform/v1/clients/{client_id}` | [修改客户端白名单与权限上限](updateClient.md) |
| DELETE | `/api/platform/v1/clients/{client_id}` | [禁用客户端并撤销其会话委托](disableClient.md) |
| GET | `/api/platform/v1/default-ui` | [读取默认界面](getDefaultUi.md) |
| PUT | `/api/platform/v1/default-ui` | [设置默认界面](setDefaultUi.md) |
