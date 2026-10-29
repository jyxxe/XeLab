# 对象权限与embedding可选依赖

[知识库目录](README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：设计契约，尚未实现。

## 默认私有表

subject_id由内核mTLS上下文提供，不接受input.owner_id或伪造头。普通接口只允许table.owner_id对应主体；父级、运维员和同角色其他用户不因关系自动读取业务表。跨用户共享ACL未在本版声明。

唯一额外身份是所有者明确批准的indexer_account_id，建议专用服务账户；它只可vector-reindex/status所需的派生维护，不能rows-query、rows-get、media-export或修改原始行。

## 核心权限组

读组包含对应table-list/get、rows-query、rows-get、vector-status、必要media-export的Skill read/invoke及目标capability.invoke；写组另含table/rows/media/vector配置操作。rows-query返回完整ID列表，rows-get按明确ID读取正文，两者分别检查权限；获得ID不等于获得行正文读取凭证。每个Skill和能力范围明确列举，不以全局通配符代替。

媒体导入需源Blob读取；导出和多模态向量需Blob创建/读取，限制在当前主体或本次资源白名单。知识库Manifest只申请权限，实际批准和最终用户授权都需满足。

## 可选模型

embedding-text和embedding-multimodal为可选依赖。无依赖时KB及普通数据能力仍Ready，不能让一个向量字段失败拖垮全部表检索。

每字段显式绑定不可变embedding来源及预批准候选；核心锁各候选安装制品、Skill定义和契约，插件再验证实际空间。文本query优先text-1，index优先text-8；只有固定获批、当前获权可用且维度/空间一致的候选可fallback，不能自行发现任意接口或跨空间切换。具体选择规则见[字段向量配置](vector-indexes.md)，调用统一经HostService，不直连推理端口。

ResolveSkill只证明当前可读取定义；无需推理的缓存/空候选路径必须使用[CheckSkillAccess](../../rpc/HostService.CheckSkillAccess.md)验证适用候选的skill.invoke及目标原动作、代理审批、当前来源链和provider就绪。同空间主备只要原预批准集合中至少一项当前满足权限、就绪及输入适配条件才可服务对应向量缓存。不能拿“可见定义”替代“可调用”，也不因检查成功跳过后续实际Invoke的输入/资源检查。

向量维护/查询的权限变化在下一次子调用和提交前重新验证。原用户被禁用后不继续用其旧句柄索引；服务任务只按独立批准的执行身份运行。

## 自动缓存的授权边界

[自动查询缓存](query-cache.md)的data/text/vector三类均按可信subject_id和原来源批准集合隔离，缓存键中的subject不接受客户端覆盖。每次命中仍重新核实当前账户/祖先/源凭证、表owner ACL、当前查询Skill及原目标动作；涉及topk或查询向量时，还校验预批准候选的当前embedding Skill/目标权限及provider可用性，不能将旧命中当作曾经授权的证明。

首选provider停用、卸载或失效时，只能检查原批准集合内同维度/空间的备用；全部适用候选不可用或无权时，旧topk/vector缓存不能继续返回成功，即使提供了原始向量、ID列表未变化或TTL尚未到期。无向量阶段的标量链继续按当前表权限正常查询，不因其他缓存不可用而停服。

query embedding的vector键不绑定本次实际主/备接口ID，可在同space、维度、purpose/prefix、规范输入和原来源批准集合相同的候选之间复用；这只是计算结果复用，不赋予新的来源调用权。data的topk缓存另保存本次所选provider审计元数据，选择变化时可保守未命中并重算。text派生结果同样校验源对象与预处理版本，不能将缓存的正文派生值暴露给无权主体。

同一账号使用不同API Key时也要执行各自固定权限上限与条件；缓存复用不复制源凭证或扩展Key动作。不同subject不共用条目，父子账户与indexer不因共享插件实例或缓存空间而共享数据。

缓存配置属于插件部署管理范围，拥有读表或查询Skill权限不自动获得调整实例内存预算的权限。三类各默认128 MB且共享实例内存，不按用户复制；任何类设0都不能把该类配额借给其他类。缓存日志、指标和命中信息不得作为枚举其他用户表/查询内容的入口。
