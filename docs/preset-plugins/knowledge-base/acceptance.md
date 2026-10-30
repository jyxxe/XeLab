# 知识库验收用例

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

1. 未安装embedding时建表、所有基本类型/文本/媒体写入、len/contains过滤成功；启用向量明确失败而不破坏数据。
2. int64极值、decimal精度、空值、数组、日期时区和超长文本按规范保存/拒绝，不经过float隐式丢精度。
3. 逐字段开启，默认128维；另一字段不建向量。输出空间一致，存储512字节/条原始向量，范数与维度校验。
4. len按Unicode码点、bytes字节、数组项数；NULL三值逻辑和typed参数精确，不用SQLite原生弱类型偷偷改变结果。
5. `a.match(len(title)>=7&&len(content)>=100).topk(...).match(...)`严格从左到右执行；后置match不向前一个topk回补，结果可少于k。topk是链式集合阶段，不是逐行bool函数；OR/NOT中的topk仍拒绝。
6. 写入与模型失败分离：原始数据已提交但向量pending/failed可诊断；默认完整索引查询拒绝遗漏，显式部分模式返回水位。
7. 重建期间行被更新/删除、模型空间改变或generation改变时，旧向量CAS提交失败，不混入当前索引。
8. 文本query默认text-1、index默认text-8；首选暂不可用后仅在固定预批准、当前获权可用、维度/空间一致的候选内fallback；明确认证/权限错误按原错误终止，不以回退掩盖撤权。全部适用候选失效则vector暂停，标量与表数据仍可用，不自动换空间或使用未批准接口。
9. 媒体源Blob过期后知识媒体仍在；导出只通过核心Blob，外部URL/宿主路径/超预算解码被拒绝。
10. 用户猜其他表UUID、父账户直接查子表、indexer调用rows-query/rows-get均拒绝；每个嵌套Skill重新鉴权，完整ID列表不自动授予正文读取权。
11. 全批行写CAS冲突不部分覆盖；reindex部分完成可核查；重启、卷满、WAL恢复和宿主外备份均可恢复。
12. 每个合法查询返回完整、有序、去重的ids/count，不以200项分页冒充完整集合。每阶段超过100000个ID、超deadline或结果过大明确失败，不返回截断成功；正文由rows-get按明确1..200个ID读取。
13. 同一链支持多个topk，最多8个、链最多32阶段；每次只处理此前阶段当前列表，并各自保存字段/分数/覆盖元数据。不同字段或query参数不串分数，平分与输入顺序规则稳定，超限拒绝。
14. reindex完成一行或ready/pending/failed/skipped状态变化，即使schema/data/generation未变也同事务递增index_revision，旧向量查询及前缀缓存失效。更换generation不重置index_revision。
15. 冷查询与自动缓存命中返回相同完整ID列表、顺序和阶段元数据；复用前缀后继续match/topk的结果等同完整重算。超时/取消/未完成阶段不能发布缓存。
16. data/text/vector三类各默认128 MB=128000000字节，总384000000字节；单条默认8MiB、固定TTL300s，每类独立LRU且命中不续期。上限和≤2147483648且≤实例memory_bytes/2，所有用户共享三类实例预算并计入4GiB，不复制每用户缓存池。
17. 任一类上限0仅关闭并清空本类，enabled=false清空三类，禁用类不向其他类借额。降低分类/单条上限先停止相关准入并淘汰后提交配置；分配失败或超限只作未命中，不放宽100000 ID硬限制、不截断结果。
18. data完整ID/前缀、text规范化/tokenization/显式chunk、vector查询向量/持久向量读取页分别计量；元数据跟随主要payload，混合对象拆分或在所有涉及类保守计量。临时准入副本/容器/LRU不得漏计，缓存淘汰不删除持久索引或暗中保留无上限正文。
19. require_complete_index=false的合法缓存保留全部阶段覆盖计数，不供true请求复用；索引进展后立即因水位失效，而非等待TTL。无向量链仍可独立正常执行。
20. 重启/备份还原后缓存为空；版本1到data_schema_version=2经显式Apply/Verify增加index_revision=1，保留既有索引代次/向量，不在Initialize偷偷迁移。最坏链式扫描仍受2vCPU/4GiB、并发与deadline约束。
21. 三类命中均重新检查当前用户/Key/表ACL与对应Skill/原目标权限；父子账户/indexer/其他subject不能借共享实例缓存读取数据。全部适用provider失效/无权时旧向量命中不复活topk，同主体不同Key仍分别检查当前上限。
22. 同空间主备切换可复用相同维度、purpose/prefix、规范输入及原来源批准集合的query embedding缓存，vector键不包含实际主/备接口ID；空间/维度/前缀/输入/批准集合变化不得误命中，仍需至少一个当前合规provider可用。data的provider审计元数据变化可保守未命中重算。

以上是待实现验收，不是已执行数据检索或推理压测报告。
