# 按字段启停向量索引

[文档总目录](../../README.md)

> 规格版本：2.6.0；状态：设计预设，尚未构建或实测运行。

Skill `org.xelab.knowledge-base.vector-configure@2.0.0`，[定义](vector-configure.skill.json)；目标 `data.kb.vector.configure@2.0.0`。由核心以固定UUID转发到org.xelab.knowledge-base，响应在SkillCallResult.call.output。

## 输入

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `table_id` | 是 | UUID |
| `expected_schema_revision` | 是 | CAS，创建/删除等另见语义 |
| `field_id` | 是 | 字段 ID，不随列重命名改变 |
| `enabled` | 是 | false 保留数据但停止向量查询与构建 |
| `dimensions` | 否 | 见完整Schema；default=128 |
| `embedding_routes` | 否 | query_skill_ids/index_skill_ids有序固定候选，allow_fallback默认true |
| `refresh_routes` | 否 | true按当前部署偏好显式刷新，与两种显式来源互斥；默认false |
| `embedding_skill_id` | 否 | 可选旧式单Skill，查询/索引同候选且不fallback |

完整嵌套校验：[input.schema.json](../../preset-plugins/knowledge-base/contracts/vector-configure/input.schema.json)。所有目标表必须属于当前用户；只有vector-reindex/status额外允许表所有者明确批准的indexer账户。

## 输出

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `table_id` | 是 | UUID |
| `schema_revision` | 是 | 新配置版本 |
| `field_id` | 是 | 字段 ID，不随列重命名改变 |
| `index` | 是 | 字段级向量索引状态 |

完整类型：[output.schema.json](../../preset-plugins/knowledge-base/contracts/vector-configure/output.schema.json)。输出投影/索引状态遵循 [知识库规范](../../preset-plugins/knowledge-base/README.md)。

## 执行与错误

变更使用RpcContext稳定幂等键，SQLite变更与安全结果同事务保存；同键不同请求冲突，CAS版本失配不覆盖。 不使用调用方任意SQL/URL，不将普通数据读写伪装成核心管理操作。

具体行为见 [接口索引](../../preset-plugins/knowledge-base/interfaces.md)，错误见 [错误目录](../../preset-plugins/knowledge-base/errors.md)。超时后先用原键/Call查询核实，不以新键重放未知写入。

候选发现、默认query=text-1/index=text-8、同空间探针、fallback及代次规则详见[embedding主备路由](../../preset-plugins/knowledge-base/embedding-routing.md)。首次省略候选使用部署偏好，已有字段省略则保留；disabled时禁止夹带路由/维度修改。
