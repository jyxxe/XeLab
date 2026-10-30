# 读取字段索引状态

[文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

Skill `org.xelab.knowledge-base.vector-status@2.0.0`，[定义](vector-status.skill.json)；目标 `data.kb.vector.status@2.0.0`。由核心以固定UUID转发到org.xelab.knowledge-base，响应在SkillCallResult.call.output。

## 输入

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `table_id` | 是 | UUID |
| `field_id` | 是 | 字段 ID，不随列重命名改变 |
| `include_failures` | 否 | 返回脱敏逐行失败原因；default=false |
| `limit` | 否 | 失败条数上限；default=50 |
| `cursor` | 否 | 失败列表游标 |

完整嵌套校验：[input.schema.json](../../preset-plugins/knowledge-base/contracts/vector-status/input.schema.json)。所有目标表必须属于当前用户；只有vector-reindex/status额外允许表所有者明确批准的indexer账户。

## 输出

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| `index` | 是 | 字段级向量索引状态 |
| `failures` | 是 | 错误记录 |
| `next_cursor` | 是 | 见完整Schema |

完整类型：[output.schema.json](../../preset-plugins/knowledge-base/contracts/vector-status/output.schema.json)。输出投影/索引状态遵循 [知识库规范](../../preset-plugins/knowledge-base/README.md)。

## 执行与错误

纯查询，无业务写入。 不使用调用方任意SQL/URL，不将普通数据读写伪装成核心管理操作。

具体行为见 [接口索引](../../preset-plugins/knowledge-base/interfaces.md)，错误见 [错误目录](../../preset-plugins/knowledge-base/errors.md)。超时后先用原键/Call查询核实，不以新键重放未知写入。
