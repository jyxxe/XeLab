# storage 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/api/platform/v1/volumes` | [列出可见卷](listVolumes.md) |
| POST | `/api/platform/v1/volumes` | [创建有硬配额的持久卷](createVolume.md) |
| GET | `/api/platform/v1/volumes/{volume_id}` | [读取卷状态和用量](getVolume.md) |
| DELETE | `/api/platform/v1/volumes/{volume_id}` | [显式销毁未挂载数据卷](deleteVolume.md) |
| POST | `/api/platform/v1/volumes/{volume_id}/resize` | [扩容数据卷](resizeVolume.md) |
| GET | `/api/platform/v1/backups` | [列出备份](listBackups.md) |
| POST | `/api/platform/v1/backups` | [创建宿主外一致性备份](createBackup.md) |
| GET | `/api/platform/v1/backups/{backup_id}` | [读取备份清单](getBackup.md) |
| POST | `/api/platform/v1/restores` | [恢复备份到新卷](restoreBackup.md) |
| GET | `/api/platform/v1/secrets` | [列出密钥元数据](listSecrets.md) |
| POST | `/api/platform/v1/secrets` | [写入托管密钥](createSecret.md) |
| GET | `/api/platform/v1/secrets/{secret_id}` | [读取密钥元数据](getSecretMetadata.md) |
| PUT | `/api/platform/v1/secrets/{secret_id}` | [轮换托管密钥内容](rotateSecret.md) |
| DELETE | `/api/platform/v1/secrets/{secret_id}` | [撤销密钥和后续租约](revokeSecret.md) |
| POST | `/api/platform/v1/secret-bindings` | [授权工作负载使用密钥](bindSecret.md) |
| GET | `/api/platform/v1/secret-bindings` | [列出可见密钥绑定](listSecretBindings.md) |
| GET | `/api/platform/v1/secret-bindings/{binding_id}` | [读取密钥绑定及版本](getSecretBinding.md) |
| DELETE | `/api/platform/v1/secret-bindings/{binding_id}` | [撤销密钥工作负载绑定](revokeSecretBinding.md) |
| POST | `/api/platform/v1/blobs` | [上传不可变业务对象](uploadBlob.md) |
| GET | `/api/platform/v1/blobs/{blob_id}` | [读取对象元数据](getBlob.md) |
| DELETE | `/api/platform/v1/blobs/{blob_id}` | [删除业务对象](deleteBlob.md) |
| GET | `/api/platform/v1/blobs/{blob_id}/content` | [下载已授权对象](downloadBlob.md) |
