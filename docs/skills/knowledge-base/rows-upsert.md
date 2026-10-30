# 原子写入或更新行

[文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

Skill `org.xelab.knowledge-base.rows-upsert@2.0.0`，[定义](rows-upsert.skill.json)；目标 `data.kb.rows.upsert@1.0.0`。由核心以固定UUID转发到org.xelab.knowledge-base，响应在SkillCallResult.call.output。

## 输入

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `table_id` | 是 | UUID |
| `expected_schema_revision` | 是 | CAS，创建/删除等另见语义 |
| `mode` | 否 | replace 全量替换、patch 只改给定列；default="replace" |
| `rows` | 是 | 同表最多64行 |

完整嵌套校验：[input.schema.json](../../preset-plugins/knowledge-base/contracts/rows-upsert/input.schema.json)。所有目标表必须属于当前用户；只有vector-reindex/status额外允许表所有者明确批准的indexer账户。

## 输出

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `table_id` | 是 | UUID |
| `data_revision` | 是 | 新水位 |
| `rows` | 是 | 完整提交行 |
| `pending_vector_fields` | 是 | 发生向量待更新的字段 |

完整类型：[output.schema.json](../../preset-plugins/knowledge-base/contracts/rows-upsert/output.schema.json)。输出投影/索引状态遵循 [知识库规范](../../preset-plugins/knowledge-base/README.md)。

## 执行与错误

变更使用RpcContext稳定幂等键，SQLite变更与安全结果同事务保存；同键不同请求冲突，CAS版本失配不覆盖。 不使用调用方任意SQL/URL，不将普通数据读写伪装成核心管理操作。

具体行为见 [接口索引](../../preset-plugins/knowledge-base/interfaces.md)，错误见 [错误目录](../../preset-plugins/knowledge-base/errors.md)。超时后先用原键/Call查询核实，不以新键重放未知写入。

## 异步批处理

批量使用[核心batch提交与查询](../batches.md)。每个item.input遵守本文Schema，202不代表已完成；逐项结果保留原业务输出。交互式查询可同步，单个写入原语内部事务与整批跨项目部分成功须分开展示。
