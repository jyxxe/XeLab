# 按ID有界读取完整投影行

[文档总目录](../../README.md) · [完整ID查询](rows-query.md)

> 规格版本：2.6.0；状态：待实现的设计契约，尚未运行读取实现。

Skill `org.xelab.knowledge-base.rows-get@1.0.0`，[定义](rows-get.skill.json)；目标`data.kb.rows.get@1.0.0`，固定插件org.xelab.knowledge-base。核心按注册的固定Skill UUID转发，业务响应位于SkillCallResult.call.output。

## 输入

| 字段 | 必填 | 约束与语义 |
|---|---|---|
| table_id | 是 | 当前用户获准知识表UUID |
| ids | 是 | 1..200个唯一行UUID，按此顺序读取；重复ID拒绝 |
| projection | 否 | 省略为全部当前列；显式列名必须存在且不重复 |
| expected_schema_revision | 否 | 必须与expected_data_revision成对 |
| expected_data_revision | 否 | 必须与expected_schema_revision成对；任一水位失配返回KB_QUERY_SNAPSHOT_EXPIRED |

完整校验：[input.schema.json](../../preset-plugins/knowledge-base/contracts/rows-get/input.schema.json)。本接口不接受expression、score、cursor、limit或order_by，不重新执行原查询或向量评分。

一次请求在同一SQLite读快照中验证表权限、投影和全部ID，随后完整返回。任一ID不存在、已删除或不属于当前表，整次404，不省略缺失ID，不返回部分成功。

## 输出

| 字段 | 必填 | 语义 |
|---|---|---|
| table_id | 是 | 目标知识表UUID |
| schema_revision | 是 | 本次快照schema水位 |
| data_revision | 是 | 本次快照数据水位 |
| items | 是 | 与输入ids一一对应、顺序一致的完整投影行数组 |

items每项必含id、revision和values；values只包含投影列并按原字段类型编码。没有score：排序和topk属于原rows-query表达式，本次仅按输入ID顺序取值。完整输出：[output.schema.json](../../preset-plugins/knowledge-base/contracts/rows-get/output.schema.json)。

## 与完整查询列表配合

rows-query一次返回完整ids；客户端可每段最多200个ID调用rows-get。每段带查询返回的schema/data作为expected_*，水位改变则失败并重新查询；不跨请求保留长读事务。

未传expected_*时读取各请求当前快照，多段结果可能来自不同数据水位。客户端必须明确选择是否接受这一点，不能把“来自同一份旧ID数组”当作历史快照保证。

## 授权、预算与错误

必须同时满足skill.invoke、目标capability.invoke和知识表owner ACL；父子账户关系或indexer身份不自动授予原始行读取。ID数组、缓存命中和已获得查询结果都不是新的授权凭证，调用时重新鉴权。

纯读取，无业务写入；单次有界响应仍受120s查询期限与核心16MiB结果上限约束，超限明确失败，不截断正文或减少items。批量读取多段时可使用[核心异步batch](../batches.md)，每项仍保持全成或全败。

稳定错误包括KB_TABLE_NOT_FOUND、KB_ROW_NOT_FOUND、KB_FIELD_NOT_FOUND、KB_QUERY_SNAPSHOT_EXPIRED、KB_VALUE_TYPE_ERROR及预算/权限错误，详见[错误目录](../../preset-plugins/knowledge-base/errors.md)。
