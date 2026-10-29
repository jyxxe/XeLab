# 链式查询执行、完整列表与读取行

[知识库目录](README.md) · [查询语法](query-language.md) · [自动缓存](query-cache.md)

> 规格版本：2.6.0；状态：待实现的设计契约；不代表已有查询执行或推理压测结果。

## 同一读快照

一次rows-query从当前SQLite读快照确定schema_revision、data_revision及各topk字段的generation/index_revision。expression先解析、类型检查并验证全部参数和当前权限，再执行根列表与各阶段；不把表达式原文直接交给SQLite。

若提供expected_schema_revision/expected_data_revision，两者必须成对，先与当前快照比较，失配返回KB_QUERY_SNAPSHOT_EXPIRED。显式input_ids全部在本表当前live行中验证，任何missing或非本表ID使整次失败，不能返回剩余行冒充完整输入。

## 顺序就是语义

默认根a按row_id升序，显式input_ids保持给定顺序；match稳定过滤，topk按当前阶段分数降序、row_id升序处理平分。最终ids沿最后阶段的顺序返回，不提供额外order_by去改变列表语义。

```text
a.match(len(title)>=7&&len(content)>=100).topk(content,:q,10).match(contains(title,:term))
```

依次产生完整根列表、完整首个match结果、最多10个topk结果、完整后置match结果。最后一步只会缩小该10项，不从被topk淘汰的候选回补。优化器只能使用不改变这些结果与顺序的实现，不跨topk移动过滤。

最多32列表阶段、8个topk，每个根/中间/最终列表最多100000 ID；不能因为后面有topk而先截断超限根列表。超限、期限到达、内存或输出预算不足均明确失败，不能返回前N条并标记为完整。

## 向量阶段

每个topk在当时输入集合上按[主备路由](embedding-routing.md)选择当前可用、已批准的query候选并验证字段、向量配置、授权和实际空间，使用同一字段generation/index_revision下有效source_hash向量。空列表或无有效候选也必须完成这些检查，不能让空输入绕过provider失效；验证通过后可跳过实际embedding。需要编码q时用受控嵌套Skill，不能保留旧调用句柄绕过撤权。

require_complete_index=true时，当前阶段候选存在pending/failed则整次报KB_VECTOR_INDEX_NOT_READY；false时只评分有效向量并明确返回覆盖元数据。NULL/空源值归skipped。各阶段计数基于其输入，不是全字段vector-status计数。

首版为精确扫描，不承诺ANN性能。float64累加float32归一化向量点积，并将舍入误差钳制在[-1,1]。同一查询有多个topk时，每阶段可用不同字段/查询向量和空间，但不能混合一个阶段内部的代次。

vector_stages按实际执行顺序返回。stage_index为整个链的0起始列表阶段序号，因此match阶段也占序号；每个topk记录field_id、generation、index_revision、dimensions、embedding_space_id、embedding_skill_id、embedding_skill_definition_digest、input_count、indexed/pending/failed/skipped行数、output_count、k、min_score和metric。

## 完整ID响应与行正文分开

rows-query响应ids/count为完整最终列表，count必须等于ids长度且无重复；无items、limit、projection、order_by或cursor。整次deadline默认受120s约束并包括排队、缓存检查、嵌套embedding和扫描；完整响应还受核心16MiB上限约束。

要取字段值，使用[rows-get](../../skills/knowledge-base/rows-get.md)，每次1..200个唯一ID，按输入顺序完整返回投影行；不存在ID整次404，不静默漏行。它是有界读取原语，没有查询游标，也不重新执行表达式或topk。

客户端可分段读取完整ids；若需维持原schema/data水位，每次rows-get成对带原expected_*，任一次失配即重新查询。此方式不跨请求保持SQLite长事务，未提供expected_*则读取各次当前快照，不能声称多请求读取是同一历史快照。

## 缓存与索引内容水位

cache_policy=auto默认允许自动缓存完整结果和中间完整列表；bypass跳过缓存读写。命中缓存仍核验当前身份、表ACL、Skill/embedding授权、依赖和所有相关水位，详见[缓存规范](query-cache.md)。

同一generation内reindex将pending变为ready也会增加index_revision；缓存不能只按schema/data/generation复用。cache返回disabled/hit/partial_hit/miss/bypass以及hit_stages/stored_stages。hit表示最终完整前缀命中，partial_hit表示仅中间前缀命中；根a不计入阶段数，表达式仅a时仍可hit且hit_stages=0。命中情况不改变查询语义或放宽资源/权限限制。

每阶段indexed_rows+pending_rows+failed_rows+skipped_rows必须等于input_count；缺失当前代次/旧source_hash向量归pending，不能遗漏候选。output_count≤min(k,indexed_rows)，score阈值可继续减少。stage_index严格递增且对应topk算子；route_digest绑定完整固定候选集合，阶段embedding_skill_id是本次可用查询候选，缓存或原始向量输入可不执行模型。cache.classes分列data/text/vector计数。
