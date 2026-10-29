# 列表操作与标量函数签名

[知识库目录](README.md) · [查询语法](query-language.md) · [执行与完整性](query-execution.md)

> 规格版本：2.6.0；状态：待实现的设计契约；rows-query@2.0.0。

## 列表操作

| 操作 | 签名 | 完整返回语义 |
|---|---|---|
| a | 当前表live行，或input_ids绑定 → IdList | 有序、去重UUID列表；显式空输入返回空列表 |
| match(predicate) | IdList × nullable bool谓词 → IdList | 在当前列表逐行计算，仅保留true，稳定保持输入顺序 |
| topk(field,:q,k[,min_score]) | IdList × 向量字段 × vector_query × int × finite number → IdList | 在当前列表中筛选有效向量、评分、阈值过滤后取至多k项；结果按cosine降序、row_id升序打破平局 |

最多32个列表阶段，topk最多8个；每阶段接收上一阶段的完整列表，不能增加先前已过滤的ID。空列表或没有有效向量候选时仍校验语法、参数类型、字段、当前权限、向量配置和provider可用性；输入验证通过后可跳过实际embedding推理。

topk的field必须是本表启用的string/text/media向量列。q只能是已绑定vector_query参数，支持显式文本、同表媒体或空间匹配的原始向量，不从普通字符串猜测模态。

k为1..100000的整数字面量或int32/int64参数；min_score为[-1,1]内有限数值字面量/参数，默认-1。每阶段实际候选仍受100000 ID预算、整次120s及16MiB输出上限约束；k不是取消资源限制的方式。

允许连续多个topk，也允许对同一字段多次topk；后一次仅对当前列表重算自己的字段分数并重新排序，不能对原全表重新取候选。各阶段使用自己的字段、查询向量、空间、索引代次和index_revision，响应vector_stages逐一记录，不把后一次分数冒充前一次分数。

列表值仅含ID，不携带可供match访问的隐式_score或其他阶段分数；按分数阈值筛选必须使用相应topk的min_score。不能将前一次topk分数当成表字段参与后续match。

`topk(...).match(...)`会留下0..k项，不回补；`match(...).topk(...)`是在先过滤的候选中取k，二者通常不同。不得跨越topk移动或合并谓词。topk不是逐行bool函数，`a.match(topk(...))`、`not topk(...)`、`topk(...) == true`均拒绝。

## match内部的标量函数

| 函数 | 参数和返回 | 语义 |
|---|---|---|
| len(x) | string/text/bytes/array/json/media → nullable int64 | 文本码点数、bytes解码字节、array项数、json数组项/对象key数、media片段数；NULL→NULL |
| contains(x,y) | 文本+文本 或 typed array+元素 → nullable bool | 区分大小写的子串或类型化元素等值；NULL参数→unknown |
| starts_with(x,y) | 文本+文本 → nullable bool | 精确前缀，不展开通配符 |
| ends_with(x,y) | 文本+文本 → nullable bool | 精确后缀 |
| is_null(x) | 任意值 → bool | NULL为true，其余false；无隐式字符串"null" |
| coalesce(a,b,...) | 2..8个同类值 → 同类型 | 首个非NULL，全NULL则NULL，不混合数字与文本 |

len(boolean/numeric/date/timestamp/uuid)及len(json标量)为类型错误，不自动stringify。array参数无元素类型时只允许len，或由明确列元素类型的contains/IN上下文校验。

## 完整性和错误

每个topk只按当前输入候选统计indexed_rows、pending_rows、failed_rows和skipped_rows；NULL/空源值记skipped，不评分也不算失败。require_complete_index默认true，任何候选存在pending/failed即报KB_VECTOR_INDEX_NOT_READY。

显式false允许仅在当前有效向量子集中完成topk，必须返回实际覆盖计数；这是调用者明确选择的候选域，并非超时后截断的假完整结果。未建向量、embedding未安装/失权、空间不匹配仍报错，不通过缓存绕过，也不降级成文本搜索或把缺向量行评分为0。
