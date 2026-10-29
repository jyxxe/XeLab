# ai 接口

[全部 HTTP 接口](../../README.md)

| 方法 | 路径 | 定义 |
|---|---|---|
| GET | `/v1/models` | [列出获授权的模型别名](listModelAliases.md) |
| GET | `/api/platform/v1/model-aliases` | [列出模型别名映射配置](listAliasMappings.md) |
| POST | `/api/platform/v1/model-aliases` | [创建获准的模型别名映射](createAliasMapping.md) |
| GET | `/api/platform/v1/model-aliases/{alias_id}` | [读取模型别名映射](getAliasMapping.md) |
| PUT | `/api/platform/v1/model-aliases/{alias_id}` | [更新模型别名映射](updateAliasMapping.md) |
| DELETE | `/api/platform/v1/model-aliases/{alias_id}` | [删除模型别名映射](deleteAliasMapping.md) |
| POST | `/v1/chat/completions` | [文本生成兼容子集](createChatCompletion.md) |
| POST | `/v1/embeddings` | [批量文本向量兼容子集](createEmbedding.md) |
