# Skill 与预设验收

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：设计预设，尚未构建或实测运行。

- 注册时目标 plugin/operation/Schema 不一致必须拒绝；同版本不同摘要不能覆盖。
- 仅有 skill.invoke、没有目标权限时拒绝，只有目标权限但试图裸调用 skill_required 时也拒绝。
- 禁用/归档 Skill 撤销新路由，已有 Call/任务能追踪原版本，不会静默切到另一预设。
- 伪造目标 URL、X-Xelab-Subject、源凭证、实例地址和 Blob 引用不能扩大权限。
- queue满、慢客户端、取消、源撤权都能释放实例槽；第9个文本8并发请求不成为第9个活动请求。
- 文本单并发和多模态单并发在多用户混合请求时都保持实例级1；容器全部线程/解码器仍受 CPU限额。
- 默认维度、字段索引配置、实际向量长度及 embedding_space_id 一致；不把768维直接截取后不归一化。
- 所有样例/清单/Skill Schema可校验，预设资源不是性能实测；正式发布须填实际模型锁并完成最坏输入压力测试。

## 异步batch与插件UI

验证202前已原子持久化，重复提交去重，单项事务与整批部分成功分明，租约接管不盲目重放unknown，结果领取重新授权。UI贡献及主题/窗口/组件验收见[WebUI验收](../preset-plugins/webui/acceptance.md)。

## 链式ID查询

验证完整ids/count无游标截断、后置match不回补、多topk按顺序执行、显式空input_ids不变成全表；rows-get按输入顺序读取全部指定行。缓存容量与索引内容修订号验收见[知识库验收](../preset-plugins/knowledge-base/acceptance.md)。
