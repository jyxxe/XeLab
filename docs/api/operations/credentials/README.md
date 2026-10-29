# credentials 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/accounts/{account_id}/api-keys` | [列出密钥元数据](listApiKeys.md) |
| POST | `/api/platform/v1/accounts/{account_id}/api-keys` | [创建部分或全部权限密钥](createApiKey.md) |
| GET | `/api/platform/v1/api-keys/{credential_id}` | [读取密钥元数据](getApiKey.md) |
| DELETE | `/api/platform/v1/api-keys/{credential_id}` | [永久吊销密钥及其派生链](revokeApiKey.md) |
| POST | `/api/platform/v1/api-keys/{credential_id}/rotate` | [签发替代密钥并吊销原密钥](rotateApiKey.md) |
