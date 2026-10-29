# 索引重建、源哈希与并发

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

## 写入与脏标记

行写入先持久化原始数据。只在被索引字段的规范值改变时标记pending，源哈希包含该字段值/媒体摘要、预处理及空间配置，不因改另一列就无谓重算。NULL/长度0文本不建向量，明确跳过。

vector-reindex按当前generation领取最多limit行，默认32、最多128。模型调用发生在SQLite事务外；提交向量时CAS校验字段当前source_hash和generation，已修改/删除的行不提交旧结果，留待新批次处理。

索引每行独立事务提交，允许一批部分完成；这不是rows-upsert的全批原子性。取消/超时后通过vector-status核实，再用新批次处理剩余脏行；已成功的相同hash/generation不会重复改变数据。

## 可见索引水位

每个kb_vector_indexes记录维护正整数index_revision。可见向量内容、空间信息或ready/pending/failed/skipped状态发生变化时，在同一SQLite事务递增对应字段的index_revision；删除向量或源行导致有效集合改变也需递增。批量事务可对同一字段合并递增一次，但不能漏记其中任一可见变化。

generation标识配置代次，index_revision标识该字段索引的可见进展。generation变化同时递增index_revision，后者单调增加，不因新generation重置；重复提交相同hash/代次且无可见变化时不必递增。data_revision不替代index_revision，因为仅完成reindex就能改变topk候选而不修改原始行。

查询读快照锁定schema/data水位和链中每个相关字段的generation/index_revision/space。多次topk按出现顺序基于当前ID列表执行，保留各阶段水位与覆盖计数；不会把最后一个字段的进展当成整条链的共同水位。

[自动缓存](query-cache.md)据上述水位失效完整列表及其前缀。显式部分索引查询也必须随进展失效，不能仅靠TTL等待更新；返回旧快照的查询也不能将其缓存键冒充当前最新水位。

## 授权寿命

不保存第一次用户请求句柄供无限后台任务。默认automatic_background_indexing=false；WebUI/SDK可在当前用户会话下写后调用小批reindex并逐批更新认证。

自动化时由核心Schedule/Task以明确批准的indexer服务账户周期调用同一Skill。table-alter/set_indexer只允许表所有者设置/撤销；indexer仍需核心Skill/embedding/Blob授权，不能作为用户请求失败时的隐式fallback。

## 状态与错误

disabled：未启用；pending/building：有待办；ready：有效源全部完成；provider_unavailable：依赖或权限不可用；failed：配置/后端故障。逐行失败存row_id/source_hash/稳定码，不在错误里存原始正文。

长文本可存储但超预设编码限额时向量失败，不静默截断或自动chunk。应用可将文档分为多行chunk后再索引；分块策略未作为隐含数据修改。

实际输出维度、空间、所有有限分量与归一化均校验后才写入。改变模型锁必须发布新预设/Skill版本，字段显式换绑后重建，不自动迁移存量向量。

SQL基线以kb_vectors的INSERT/UPDATE/DELETE触发器和索引配置变更触发器在事务内推进index_revision，保守多次递增允许。显式路由变更生成新代次，已有同空间候选间临时fallback不换代；kb_vectors记录实际embedding_skill_id/定义摘要/来源安装供审计，仍按原source_hash/generation提交。
