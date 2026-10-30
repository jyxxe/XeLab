# Skill 转发、并发、取消和兼容入口

[文档总目录](../README.md)

> 规格版本：2.8.0；状态：设计预设，尚未构建或实测运行。

## 调用步骤

1. 验证身份、祖先、源凭证及 skill.invoke，读取不可变定义。
2. 校验输入 Schema、声明 Blob 引用及当前目标动作权限。
3. 普通目标按固定安装制品、plugin_id/契约、部署约束和健康确定提供者；micro目标按固定微版本/制品/微部署及其MPR安装解析，不能因其他实例更空闲而换目标。
4. 锁定定义摘要、创建 Call/委托/根请求预留，进入目标实例的有界并发池。
5. Broker 发送受控 RPC 或声明式 HTTP，校验输出，返回 SkillCallResult。

实例并发为全部用户、全部 Skill 和工作流共享的 max_active_requests，不是每用户/每路由各一份。health/lifecycle 走独立控制处理路径，不排到业务队列尾。

## 超时和重试

整体 deadline 覆盖队列、预处理、嵌套 Skill、推理、数据库和响应。queue_timeout_ms 与调用剩余期限取更小者，队列满或到期返回 RESOURCE_EXHAUSTED/DEADLINE_EXCEEDED，不无限占用内存。

取消或来源链失效传播到所有子调用，释放队列/活动槽。数据存储类插件已提交的写入不能当作撤回；以稳定幂等键和结果查询核实。embedding side_effect=none，可按原总体期限安全重试，但不隐式换不同向量空间。

根业务调用只占一次用户日调用额度；子调用沿 root_call_id 共享根 reservation，不重复复制用户额度。每个物理子请求仍占目标实例并发与资源；单根并行 fan-out 最多16。审计与指标记录全部物理调用。

## 入口兼容

/v1/embeddings 的模型别名可绑定 skill_id。对 skill_required 提供者必须绑定，contract/provider 必须与 Skill 一致；核心经 SkillRouter 转发而非绕过。

文本适配按目标最大 batch 拆分，原输入顺序合并，统一根 deadline；兼容接口维度固定768，purpose取别名 embedding_purpose（默认 semantic_similarity）。直接 Skill 请求可明确128/256/512/768。多模态不塞入文本兼容字段，使用多模态 Skill。

工作流 step.skill_id 与 contract 二选一。核心持久化定义摘要及任务范围绑定，普通 Skill 节点通过 Broker 调用，不要求无状态推理插件实现 TaskWorker；长期 automation.execute 仍使用原执行协议。

## HTTP 目标

核心只按安装时核实的 operationId 取固定路径/方法/Schema。后端8080使用实例 mTLS，验证内核来源；X-Xelab-Delegation/X-Xelab-Subject 由核心移除外部同名头后重建。HTTP结果的虚拟 ContractRef 使用 Skill.name/version/definition_digest，Schema 权威为 Skill 定义，不冒充可裸调用的能力。

## 批处理

用户批量操作默认使用[异步batch协议](batches.md)，核心持久化后返回202并提供查询；原calls和/v1/embeddings保持同步。有界插件内部batch是单次执行原语，不等于整批用户任务。batch按每个实际执行item建立根调用及额度，子调用不重复计费。

## micro目标执行

1. 读取固定微版本/微部署，校验MicroPlugin未归档、微部署enabled、包/源码及Skill摘要一致。
2. 执行skill.invoke、具体MicroPlugin UUID上的micro.invoke，以及父runtime_deployment_id上的micro.runtime.use；核对当前祖先/源凭证、MPR部署审批及运行时安装匹配。
3. 按MicroManifest业务输入Schema验证input；用户不得覆盖微对象、版本、部署、制品摘要或准备句柄。
4. 先取得MPR父实例活动槽及执行内存预算，再取得微部署槽与受限引擎预算，任一步失败释放先前预留。
5. 缺少当前实例代次准备句柄时，由控制器通过MicroRuntimeService.Prepare准备精确制品；Invoke只使用内核构造的可信微目标及JSON输入。
6. 验证完整业务输出Schema、当前授权及固定绑定后提交Call结果；错误、取消或超限不提交部分JSON。

MicroRuntimeService只允许内核mTLS调用。Cancel/QueryResult使用CapabilityService协议，Lifecycle使用普通插件协议；这些控制入口不作为可裸调用的微业务能力。微脚本不接收父RpcDelegation、实例证书或HostService句柄。

Prepare句柄绑定MPR instance/generation及微版本/部署/制品，可丢弃重建，不是授权。父MPR重启或停用即失效；路由等待同一固定父部署恢复，不能漂移到其他安装。MicroDeployment自身无OCI实例，不重复执行节点Admission预留。

微停用/归档、来源撤权和父实例停止均取消在途纯计算并释放预算；运行时停用撤销新路由，Release幂等清理句柄。batch依赖不可用等待固定运行时，结果未知/重试继续遵守原任务规则，不让安全重算变成换版本重跑。

裸CapabilityService.Invoke、HostService.Invoke、工作流裸contract或用户直连MPR不得绕过micro Skill绑定。调用方只给业务input，普通核心转发身份头和其他JSON字段不能冒充内核微调用上下文。详见[微插件架构](../architecture/micro-plugins.md)。

## 扩展转发

extension目标经核心解析固定宿主链及物理根当前代次；ExtensionHostService.Prepare/Invoke/Release只发送至真实根OCI，不为逻辑扩展建立内网地址。输入、输出与副作用受固定扩展点及候选Schema约束，返回前仍校验当前授权和结果证据。

data只加载有界只读贡献；skill处理器固定已注册Skill与定义摘要，经核心转发并继承剩余deadline、取消、根调用计数和幂等上下文。handler独立服务按自己原资源准入，不能在宿主内隐式执行；handler目标也参与统一DAG。根重启或宿主链失效时等待同一精确链重新Prepare，不转向latest、不借裸能力接口绕过extension授权。详见[扩展Skill](extensions/README.md)。
