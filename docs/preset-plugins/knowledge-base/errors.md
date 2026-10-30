# 知识库稳定错误与恢复

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

| code | HTTP / gRPC | 处理 |
|---|---|---|
| KB_TABLE_NOT_FOUND / KB_FIELD_NOT_FOUND / KB_ROW_NOT_FOUND | 404 / NOT_FOUND | 不泄漏其他主体对象 |
| KB_SCHEMA_CONFLICT / KB_ROW_CONFLICT | 409 / ABORTED | 读当前版本后显式重试 |
| KB_VALUE_TYPE_ERROR | 400 / INVALID_ARGUMENT | 修正字段类型/空值/精度 |
| KB_QUERY_PARSE_ERROR / KB_QUERY_TYPE_ERROR | 400 / INVALID_ARGUMENT | 修正有界表达式或typed参数 |
| KB_QUERY_SNAPSHOT_EXPIRED | 409 / FAILED_PRECONDITION | 从第一页重新查询 |
| KB_VECTOR_NOT_ENABLED / KB_VECTOR_INDEX_NOT_READY | 409 / FAILED_PRECONDITION | 配置或重建；允许部分结果须显式声明 |
| KB_EMBEDDING_SPACE_MISMATCH | 409 / FAILED_PRECONDITION | 选择匹配空间或重新建索引，不截错维度 |
| KB_EMBEDDING_UNAVAILABLE | 503 / UNAVAILABLE | 普通查询仍可用；恢复批准的provider |
| KB_MEDIA_INVALID | 400 / INVALID_ARGUMENT | 格式/模态/大小不符 |
| KB_STORAGE_FULL | 503 / RESOURCE_EXHAUSTED | 拒绝新写并保留已提交数据，扩容后重试 |
| KB_QUERY_BUDGET_EXCEEDED | 503 / RESOURCE_EXHAUSTED | 缩小候选，不能返回假完整topk |

PERMISSION_DENIED、DEADLINE_EXCEEDED、INTERNAL沿用核心规则。稳定码由已签名契约登记并按标准gRPC状态映射HTTP，用户消息脱敏；不允许任意插件伪造核心身份错误或原样泄漏SQL/正文。

标量查询无业务副作用。派生索引有独立CAS去重，可核查状态后补齐；原始行写入超时需先用原幂等键/Call查询核实。模型错误不能把原始业务行回滚为“从未写入”。

## 链式列表与正文读取

KB_QUERY_RESULT_LIMIT_EXCEEDED表示根/中间/最终ID列表超过100000个硬上限；返回409的业务状态错误，整次不返回截断ids。全表根过大时可显式提供受限input_ids选择候选，不能默认先切片。AST/候选/时间等仍使用KB_QUERY_BUDGET_EXCEEDED或平台资源/超时错误。

expected_schema_revision/data_revision必须成对，失配为KB_QUERY_SNAPSHOT_EXPIRED。input_ids或rows-get.ids中的任何missing/非本表ID为KB_ROW_NOT_FOUND，整次404；旧filter/limit/projection/cursor等字段在rows-query@2.0.0为INVALID_ARGUMENT。缓存未命中或容量不足不是业务错误；查询本身仍执行硬上限检查。
