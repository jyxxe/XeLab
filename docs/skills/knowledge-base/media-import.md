# 持久导入受控媒体

[文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

Skill `org.xelab.knowledge-base.media-import@2.0.0`，[定义](media-import.skill.json)；目标 `data.kb.media.import@1.0.0`。由核心以固定UUID转发到org.xelab.knowledge-base，响应在SkillCallResult.call.output。

## 输入

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `table_id` | 是 | UUID |
| `kind` | 是 | 期望模态，必须与实际媒体一致 |
| `blob_id` | 是 | 获准源 Blob |

完整嵌套校验：[input.schema.json](../../preset-plugins/knowledge-base/contracts/media-import/input.schema.json)。所有目标表必须属于当前用户；只有vector-reindex/status额外允许表所有者明确批准的indexer账户。

## 输出

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `id` | 是 | UUID |
| `table_id` | 是 | UUID |
| `kind` | 是 | 模态 |
| `content_type` | 是 | 嗅探并验证的 MIME |
| `size_bytes` | 是 | 原始内容长度 |
| `digest` | 是 | SHA-256 |
| `reference_count` | 是 | 引用行数 |

完整类型：[output.schema.json](../../preset-plugins/knowledge-base/contracts/media-import/output.schema.json)。输出投影/索引状态遵循 [知识库规范](../../preset-plugins/knowledge-base/README.md)。

## 执行与错误

变更使用RpcContext稳定幂等键，SQLite变更与安全结果同事务保存；同键不同请求冲突，CAS版本失配不覆盖。 不使用调用方任意SQL/URL，不将普通数据读写伪装成核心管理操作。

具体行为见 [接口索引](../../preset-plugins/knowledge-base/interfaces.md)，错误见 [错误目录](../../preset-plugins/knowledge-base/errors.md)。超时后先用原键/Call查询核实，不以新键重放未知写入。
