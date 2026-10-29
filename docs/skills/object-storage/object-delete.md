# CAS删除本人对象

[对象存储Skill目录](README.md) · [插件接口](../../preset-plugins/object-storage/interfaces.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

Skill `org.xelab.object-storage.object-delete@1.0.0`，[定义](object-delete.skill.json)；目标`data.object.delete@1.0.0`。响应位于`SkillCallResult.call.output`。

## 输入

| 字段 | 必填 | 类型与约束 |
|---|---|---|
| `key` | 是 | 当前subject的完整合法逻辑key |
| `expected_revision` | 是 | 正整数，必须匹配待删除对象的当前revision |

完整定义：[input.schema.json](../../preset-plugins/object-storage/contracts/object-delete/input.schema.json)。单次只删除一个明确key，不接受通配符、prefix、owner或递归目录参数。

## 输出

| 字段 | 类型 | 定义 |
|---|---|---|
| `object_id` | UUID字符串 | 被删除对象的ID |
| `deleted_revision` | 正整数 | 本次删除推进后的head水位，即删除前可见revision+1 |

完整定义：[output.schema.json](../../preset-plugins/object-storage/contracts/object-delete/output.schema.json)。成功表示对象已在事务中隐藏，后续stat/list不可见，用户逻辑用量已扣减且幂等结果已保存；不表示底层全部物理字节已被擦除。

## 幂等与生命周期

本接口side_effect=idempotent，使用稳定业务键。成功重放先于当前对象存在性/CAS检查，返回原object_id/deleted_revision。即使同key后来已重新create，也不能因原删除请求重放而删除新对象。

删除保留永久head/tombstone；同key重建分配新object_id并使用deleted_revision+1。旧expected_revision不再匹配新对象，不能因新对象恰好也从1开始而误删；head元数据仍计入部署/卷物理预算，不随内容GC回收。

首次执行时key不存在为RESOURCE_NOT_FOUND；revision不匹配或同键不同指纹为STATE_CONFLICT；key格式非法为INVALID_ARGUMENT。对未知执行结果使用原键/原Call查询，不用新键自动删除可能已改变的对象。

已打开读取可通过内部pin完成，内容无引用后由有界GC回收；实际回收前继续占部署/卷空间。删除源对象不撤销已合法创建的核心Blob、已下载副本或外部备份，不能承诺立即物理擦除。

大量明确对象删除使用[核心异步batch](../batches.md)，每项独立CAS，允许部分成功，无整个批次原子删除。插件禁用或卸载本身不等价于本接口，不自动删除业务卷。
