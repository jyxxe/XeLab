# 表、列与行的事务语义

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

## 命名与结构

表名和列名采用ASCII字母开头、后续字母数字下划线，最长63，大小写敏感。下划线开头留给系统。每表1..128列，字段有稳定UUID，重命名不改变field_id。

table.schema_revision覆盖列/索引配置，data_revision覆盖可见行数据水位；row.revision覆盖该行变更。客户端不得自行写owner、revision或索引状态。

table-alter只定义add_field、rename_field、drop_field和set_indexer。修改类型/nullable/default需另行迁移版本，不通过本接口隐式转换。删列必须confirm_drop_data=true，关联向量随该字段停用并按GC清理。

新增不可空列必须给有效default。新增列可采用虚拟默认值投影，在写入时物化；schema变化立即失效旧游标并调整可见数据水位，不要求单事务重写所有历史行。

## 行写入

expected_revision=0表示必须新建，>0表示精确当前行版本。新建可省略id，由服务端分配并在幂等结果中保存；更新必须提供id。相同批内重复row_id拒绝。

replace：缺列使用字段default，若无default则nullable列置null，不可空列报错。patch：已有行只改给定列；新建仍完整校验必填列。未知列拒绝，不保存到隐藏extra对象。

schema/行CAS、类型、媒体引用与quota全部校验后，同一SQLite事务提交整个≤64行批次。行和data_revision各递增一次，向量源字段变化标记pending；不在事务里执行远程embedding。

## 权限、删除和数据可用性

默认仅table.owner_id对应主体读写，父子账户关系不自动开放数据。显式indexer只可维护派生向量，不能查询或导出原始行。

删除行同事务使向量无效并递减媒体引用。table-drop销毁该表元数据/行/向量/媒体引用，必须明确确认ID；原核心导出Blob遵守各自TTL，不假装删除本地行即可撤回所有已下载副本。

销毁先原子标记业务不可见，私有文件/旧向量的物理清理由有界GC完成；既有读快照/备份的保留范围需明确，不能声称立即擦除全部磁盘历史。列/表名的唯一约束仅针对未删除对象，重建同名对象获得新UUID。

embedding不存在或失败不影响普通行写入；字段向量保留pending/failed供修复，不把写入成功伪称已完成向量化。

## 异步导入边界

用户批量导入通过[核心异步batch](../../skills/batches.md)拆为多个≤64行upsert项目；每项保持上述原子性，整批不承诺跨项目回滚。提交202后通过batch_id/item.id查询成功、失败、取消或未知结果。
