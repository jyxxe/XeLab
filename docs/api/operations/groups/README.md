# groups 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/permission-groups` | [列出可见权限组](listGroups.md) |
| POST | `/api/platform/v1/permission-groups` | [建立个人或公共权限组](createGroup.md) |
| GET | `/api/platform/v1/permission-groups/{group_id}` | [读取权限组](getGroup.md) |
| DELETE | `/api/platform/v1/permission-groups/{group_id}` | [归档组并撤销其有效绑定](archiveGroup.md) |
| POST | `/api/platform/v1/permission-groups/{group_id}/versions` | [发布不可变组版本](publishGroupVersion.md) |
| GET | `/api/platform/v1/permission-groups/{group_id}/versions/{version_id}` | [读取权限组版本](getGroupVersion.md) |
| GET | `/api/platform/v1/accounts/{account_id}/grants` | [列出账户授权来源](listGrants.md) |
| POST | `/api/platform/v1/accounts/{account_id}/grants` | [绑定角色或组版本](createGrant.md) |
| GET | `/api/platform/v1/grants/{grant_id}` | [读取单项授权及版本](getGrant.md) |
| DELETE | `/api/platform/v1/grants/{grant_id}` | [撤销单一授权来源](revokeGrant.md) |
