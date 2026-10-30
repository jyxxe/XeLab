# 查询与缓存配置

[知识库目录](README.md) · [默认JSON](defaults.json) · [清单Schema](manifest.json) · [缓存语义](query-cache.md)

> 规格版本：2.8.0；状态：待实现的配置契约。

配置通过已有核心updateDeployment管理，最终传入插件Initialize；只持有读表/查询权限不能改实例设置。当前生命周期按新部署代次重建实例应用配置，旧缓存随旧进程清空，新实例先使用新上限，不额外假设存在未定义的热重载RPC。

| 字段 | 默认/当前上限 | 定义 |
|---|---|---|
| maximum_query_bytes | 8192 | 表达式UTF-8字节，不按字符数放宽 |
| maximum_ast_nodes/depth | 256/32 | 受控AST预算 |
| maximum_query_ids | 100000 | 根、中间和最终列表上限；超限报错不截断 |
| maximum_row_fetch_ids | 200 | rows-get每次完整返回的请求ID数 |
| maximum_query_stages | 32 | 列表链算子数，不含根a |
| maximum_topk_stages | 8 | 同一链最多topk数，允许同字段多次 |
| maximum_vector_candidates | 100000 | 每个topk的输入候选硬上限 |
| maximum_expression_in_items | 1000 | match内IN展开上限 |
| maximum_query_time_ms | 120000 | 整次查询包括排队、缓存/授权检查、推理与扫描 |
| maximum_active_requests | 8 | 全实例共享业务请求数，仍受manifest并发准入 |
| automatic_background_indexing | false | 不因启用查询缓存自动触发后台embedding |

上述执行预算在当前Schema中为const；更改需修改契约和资源验收，不通过输入临时放宽。query_cache字段为可配置范围，不使用const锁死容量：

```json
{"query_cache":{"enabled":true,"data_max_bytes":128000000,"text_max_bytes":128000000,"vector_max_bytes":128000000,"max_entry_bytes":8388608,"ttl_seconds":300}}
```

data_max_bytes/text_max_bytes/vector_max_bytes分别为0..2147483648整数，各默认128000000字节（SI128 MB），某类0只禁用并清空本类；三类合计384000000字节，不借额。三者之和必须≤2147483648且≤实例memory_bytes/2；Schema检查单字段，插件Initialize在分配前检查跨字段与资源约束，非法配置拒绝就绪，不以超额缓存运行。max_entry_bytes为1..16777216，默认8MiB；ttl_seconds为1..86400，默认300，命中不续期。

减少容量或禁用必须先让缓存收敛到新限额再确认应用成功。现有recreate路径通过停止旧实例/释放其内存后启动新实例满足这一点；若未来增加热配置也必须遵守相同收敛规则，并另外定义接口，不能先承诺新上限而继续使用旧缓存。

缓存不持久化，不修改20GiB业务卷预算；行、媒体、向量仍持久存储。cache_policy=auto/bypass只控制当前查询是否读写缓存，不修改实例全局容量，也不授予部署管理权。

embedding_routing分别提供text_query_preference（默认text-1→text-8→multimodal-1）、text_index_preference（text-8→text-1→multimodal-1）和multimodal_preference，以及allow_fallback=true。仅字段首次配置/显式refresh时解析已批准固定候选，详见[主备路由](embedding-routing.md)。
