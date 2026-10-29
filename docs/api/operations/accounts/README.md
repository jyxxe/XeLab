# accounts 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/accounts` | [列出可管理或可见账户](listAccounts.md) |
| GET | `/api/platform/v1/accounts/{account_id}` | [读取账户](getAccount.md) |
| PATCH | `/api/platform/v1/accounts/{account_id}` | [更新或禁用账户](updateAccount.md) |
| DELETE | `/api/platform/v1/accounts/{account_id}` | [归档账户](archiveAccount.md) |
| POST | `/api/platform/v1/accounts/{account_id}/children` | [创建子账户](createChildAccount.md) |
| POST | `/api/platform/v1/accounts/{account_id}/password` | [本人修改密码并撤销旧会话](changePassword.md) |
| PUT | `/api/platform/v1/accounts/{account_id}/inheritance` | [调整子账户继承范围](updateInheritance.md) |
| GET | `/api/platform/v1/accounts/{account_id}/effective-permissions` | [查询有效权限](getEffectivePermissions.md) |
| POST | `/api/platform/v1/authorization/check` | [检查本人操作权限](checkAuthorization.md) |
| GET | `/api/platform/v1/roles` | [列出内置角色](listRoles.md) |
| GET | `/api/platform/v1/permission-definitions` | [列出可见权限目录](listPermissions.md) |
| GET | `/api/platform/v1/accounts/{account_id}/quota` | [读取账户树配额](getQuota.md) |
| PUT | `/api/platform/v1/accounts/{account_id}/quota` | [调整账户树额度](updateQuota.md) |
