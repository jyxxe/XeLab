# 完整ID列表链式查询

[文档总目录](../../README.md) · [查询语言](../../preset-plugins/knowledge-base/query-language.md)

> 规格版本：2.6.0；状态：待实现的设计契约，尚未运行查询实现。

Skill `org.xelab.knowledge-base.rows-query@2.0.0`，[定义](rows-query.skill.json)；目标`data.kb.rows.query@2.0.0`，固定插件org.xelab.knowledge-base。核心按固定Skill UUID转发，业务响应位于SkillCallResult.call.output。

## 输入

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| table_id | 是 | 当前用户获准知识表UUID |
| expression | 否 | 默认`a`；完整ID列表链式表达式，UTF-8≤8192字节 |
| input_ids | 否 | 显式根列表，最多100000；保持输入顺序，空数组表示空列表；省略表示当前表全部live行 |
| params | 否 | 命名类型化参数，名称不含冒号；必须与表达式实际使用一致 |
| expected_schema_revision | 否 | 必须与expected_data_revision成对，匹配本次快照 |
| expected_data_revision | 否 | 必须与expected_schema_revision成对；失配为KB_QUERY_SNAPSHOT_EXPIRED |
| require_complete_index | 否 | 默认true，任一topk当前候选中有pending/failed向量时拒绝 |
| cache_policy | 否 | auto（默认）或bypass；bypass不读写自动缓存 |

完整校验：[input.schema.json](../../preset-plugins/knowledge-base/contracts/rows-query/input.schema.json)。表达式仅允许a、match和topk链；每阶段完整返回有序去重ID，最多32阶段/8个topk，k=1..100000。match后置于topk时不回补，不允许优化器重排这两个阶段。

显式输入任何ID不存在、已删除或不属于本表，整次失败，不过滤后悄悄继续。表仍按插件owner ACL检查；indexer身份不因获准维护向量就能rows-query。传入ID不是读取授权。

## 输出

| 字段 | 必填 | 语义 |
|---|---|---|
| table_id | 是 | 目标表UUID |
| schema_revision | 是 | 实际只读快照schema水位 |
| data_revision | 是 | 实际只读快照数据水位 |
| ids | 是 | 完整最终UUID列表，最多100000，顺序遵循链式操作 |
| count | 是 | 必须等于ids长度 |
| vector_stages | 是 | 每个topk的实际执行与覆盖元数据；无topk为[] |
| cache | 是 | 自动缓存状态与命中/存储阶段计数 |

vector_stages每项包含stage_index（整个链列表阶段从0计数）、field_id、generation、index_revision、dimensions、embedding_space_id、embedding_skill_id、embedding_skill_definition_digest、input_count、indexed_rows、pending_rows、failed_rows、skipped_rows、output_count、k、min_score和metric。多个topk逐一记录，不能只返回最后一阶段元数据。

cache.state为disabled/hit/partial_hit/miss/bypass，另有hit_stages与stored_stages。hit为最终完整前缀命中，partial_hit为中间前缀命中；根a不计阶段数，表达式仅a可hit且hit_stages=0。完整结构：[output.schema.json](../../preset-plugins/knowledge-base/contracts/rows-query/output.schema.json)，缓存规则见[query-cache](../../preset-plugins/knowledge-base/query-cache.md)。

## 完整性、正文读取与错误

响应不分页，不含items/cursor/projection/order_by/limit。根、中间或最终列表超过100000、执行超过120s或完整输出超过16MiB，明确返回预算/超时错误，不截断为“成功”。

需要正文时调用[rows-get](rows-get.md)，每次1..200个唯一ID；需要相同schema/data水位时成对传expected_*。把ids重新传为另一查询的input_ids是按值输入，默认读取新的当前快照，不保留旧SQLite事务；vector_stages描述历史查询使用的向量水位，不承诺跨请求保留向量版本。

所有调用重新检查Skill和目标能力权限、表ACL及嵌套embedding权限；缓存命中不能旁路。语法、类型、索引完整性、空间和水位错误见[知识库错误](../../preset-plugins/knowledge-base/errors.md)。

## 异步batch

大量独立表达式通常使用[核心异步batch](../batches.md)。每个item.input为上述完整请求，每个成功item仍返回完整最终ID列表及其水位；202只表示已接收。交互式单次查询可同步，不能通过batch偷偷截断单项结果。

缓存诊断cache.classes按data/text/vector给出本请求hits/stores；三类缓存各默认128 MB、独立上限。vector_stages还包含route_digest；embedding_skill_id表示本次按主备路由检查的查询候选，不保证实际发生模型执行。重复input_ids拒绝，标量match顺序不因缓存而改变。
