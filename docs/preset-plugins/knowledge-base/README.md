# 知识库预设插件

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

插件ID `org.xelab.knowledge-base`。提供数据表、类型化行、媒体存储、完整ID列表链式查询和可选字段向量检索。核心经Skill转发，插件不开放未经授权的直连端口。

## 默认建议

2vCPU/4GiB、单副本、20GiB独占持久卷、1GiB临时盘、活动业务并发8；SQLite WAL，单写入事务。上述是待测初始配置，用户尚未指定知识库硬件要求，可按验收调整。

普通存储、contains/len等标量查询不依赖embedding插件。每个字段默认不建向量；启用后默认128维float32+L2归一化+cosine。128维原始向量每行每字段512字节，另加索引/版本/SQLite开销。

文本列可选择两种文本embedding预设；media列必须选多模态Skill。安装一个embedding并不等于给所有表、字段或用户自动开启向量。

## 文档

- [接口与Skill](interfaces.md)、[类型](data-types.md)、[表与行](tables-rows.md)
- [查询语言](query-language.md)、[函数](query-functions.md)、[查询执行与完整ID列表](query-execution.md)、[自动缓存](query-cache.md)
- [字段向量](vector-indexes.md)、[查询/索引embedding主备路由](embedding-routing.md)、[重建与一致性](index-consistency.md)
- [媒体持久化](media.md)、[身份与可选依赖](authorization.md)
- [存储](storage.md)、[错误](errors.md)、[验收](acceptance.md)
- [清单](manifest.json)、[配置说明](configuration.md)、[配置](defaults.json)、[部署模板](deployment.template.json)、[完整类型Schema](types.schema.json)

当前没有任意SQL、脚本函数、向量ANN近似结果或通用跨用户表共享ACL接口；Owner/显式indexer的业务隔离规则在本插件内执行。

## 链式查询与自动缓存

```text
a.match(len(title)>=7&&len(content)>=100).topk(content,:q,10).match(contains(title,:term))
```

每个列表阶段产生完整有序ID列表，严格按链顺序处理，后置match不补足topk。rows-query@2.0.0返回完整ids/count，正文用rows-get按需读取，不把200行分页当完整查询。

自动内存缓存默认启用，数据、文本、向量三类各默认128 MB（128000000字节），分别通过query_cache.data_max_bytes/text_max_bytes/vector_max_bytes配置，某类设0只关闭该类；单条默认8MiB，固定TTL默认300秒。缓存仍核验当前权限、数据水位和字段index_revision，不因命中跳过provider检查。

本版知识库插件2.0.0、data_schema_version=2。原14个Skill以2.0.0固定新安装，新增rows-get@1.0.0；旧查询filter/投影/游标契约不在新版本中隐式保留。

文本查询默认优先text-1，向量建立默认优先text-8，多模态走multimodal-1；可在已验证同空间、当前获权的固定候选内fallback。部署偏好变更不会自动改写既有字段路由，详见[主备选择](embedding-routing.md)。
