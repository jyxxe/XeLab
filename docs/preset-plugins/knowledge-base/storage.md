# 知识库存储、持久卷与恢复

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

采用独占/data/kb卷，建议20GiB起步；SQLite WAL，数据库、媒体、向量和幂等/诊断都计入卷配额，临时导入另外计入临时盘。多副本默认禁止同时写同一卷。

[storage.sql](storage.sql)为插件私有逻辑结构，不是核心PostgreSQL表。core账号/凭证/角色不写入此库，只有经过验证的owner/indexer标识及业务对象权限。

cells_json按field_id存储，对外按当前字段名投影，重命名不改历史值含义。int64/decimal用规范字符串，media为持久引用，向量为128等维float32 little-endian二进制；不能依赖SQLite字符串比较模拟类型比较。

幂等请求先记指纹，原子业务写入与安全结果同SQLite事务。reindex每行独立提交并记录进度；崩溃后按source_hash/generation核实。只读临时Blob输出按TTL管理，不把幂等元数据当密钥或永久文件缓存。

向量字段持久化generation和单调index_revision；后者覆盖逐行向量内容/状态的可见进展，与索引更新同事务提交，详见[索引一致性](index-consistency.md)。原始行data_revision不替代该索引水位。

[查询自动缓存](query-cache.md)分为data完整ID/排序/阶段元数据、text纯文本读取/显式预处理与chunk结果，以及vector查询embedding/持久向量只读派生读取页，三类各默认128 MB=128000000字节，共384000000字节。三类独立上限及LRU，不互借配额；总和还需≤2147483648且≤实例memory_bytes/2，计入插件4GiB内存。

三类缓存不写SQLite或业务卷，也不包含在备份内；text中的必要文本副本受本类预算约束，不能隐藏成无上限正文副本。vector派生读取页与SQLite持久业务向量索引分开计量，淘汰缓存不删除源行或持久索引。重启/升级/还原后从空缓存开始，不能恢复旧授权上下文或错误复用旧水位。

备份先停止写入/检查点WAL并建立媒体一致性清单，再由核心卷备份复制到宿主外。只备份数据库文件而忽略WAL/媒体不能声称可恢复。还原后校验SQLite完整性、媒体摘要、字段空间、索引generation/index_revision及对应状态，再Ready。

data_schema_version=2。从版本1迁移时为已有索引记录增加index_revision并初始化为1，保留原generation、向量内容和状态；新记录也从正整数初值1开始。迁移/Verify核对字段存在、非零及写入递增规则，不借迁移静默重算全部向量。

迁移遵守平台recreate/独占卷/备份/Apply/Verify流程；不能在Initialize隐式修改持久格式。升级失败先明确版本1/2兼容及恢复方案，不能只回退二进制。查询缓存不属于需迁移的业务数据。

版本2还持久化字段embedding_routes_json/route_digest和向量实际来源Skill/安装；迁移保留旧单Skill为两用途同一候选且禁用fallback，经固定安装/定义/空间验证后才服务。无法核实的旧字段保持provider_unavailable，要求所有者显式配置，不自动发现替换或丢弃历史向量。
