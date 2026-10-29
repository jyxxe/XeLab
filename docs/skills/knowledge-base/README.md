# 知识库业务Skill

[文档总目录](../../README.md)



- [导出媒体为临时核心Blob](media-export.md)
- [持久导入受控媒体](media-import.md)
- [删除明确行集合](rows-delete.md)
- [完整ID列表链式检索](rows-query.md)
- [按ID读取行正文](rows-get.md)
- [原子写入或更新行](rows-upsert.md)
- [修改知识表结构](table-alter.md)
- [创建知识表](table-create.md)
- [销毁知识表和所属数据](table-drop.md)
- [读取知识表结构](table-get.md)
- [列出本人知识表](table-list.md)
- [按字段启停向量索引](vector-configure.md)
- [明确删除字段向量数据](vector-drop.md)
- [有界批量更新待索引行](vector-reindex.md)
- [读取字段索引状态](vector-status.md)

## 批处理入口

批量写入、删除、检索和有界reindex通常使用[核心异步batch](../batches.md)。每项input遵守下列原Skill定义；upsert/delete单项最多64行原子提交，跨项目允许部分成功。交互式检索保持同步；WebUI展示见[任务中心](../../preset-plugins/webui/batches.md)。

## ID列表与缓存

[链式查询语言](../../preset-plugins/knowledge-base/query-language.md)以a为完整ID列表，match和topk可顺序组合。rows-query返回ids/count；要取正文另用rows-get。实例[自动缓存](../../preset-plugins/knowledge-base/query-cache.md)数据、文本、向量三类各默认128 MB，可分别配置上限，命中不绕过授权或索引内容水位。
