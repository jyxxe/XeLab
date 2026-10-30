# Embedding查询与索引路由

[知识库目录](README.md) · [字段向量](vector-indexes.md) · [模型锁](../embedding-gemma-model.md) · [权限](authorization.md)

> 规格版本：2.8.0；状态：待实现的设计契约。候选可用性和资源数值不是已运行性能结果。

## 两种用途分别选优先项

知识库将查询向量编码与数据索引向量建立分开路由。文本查询默认优先text-1，文本索引默认优先text-8，多模态输入默认使用multimodal-1；不是把一个全局默认模型强加给所有用途。

部署配置embedding_routing提供以下初始发现偏好：

| 配置 | 从左到右的默认plugin ID顺序 |
|---|---|
| text_query_preference | org.xelab.embedding-gemma.text-1 → org.xelab.embedding-gemma.text-8 → org.xelab.embedding-gemma.multimodal-1 |
| text_index_preference | org.xelab.embedding-gemma.text-8 → org.xelab.embedding-gemma.text-1 → org.xelab.embedding-gemma.multimodal-1 |
| multimodal_preference | org.xelab.embedding-gemma.multimodal-1 |
| allow_fallback | true |

这些plugin ID只用于字段首次配置或显式刷新时发现已经安装、批准、注册的候选Skill；不是运行时按plugin ID或latest解析端点。修改部署偏好也不悄悄改写已有字段候选。

发现通过HostService.ListSkillCandidates，按明确plugin_ids和capability_names分页读取当前可见的固定版本；它只做发现，不证明invoke权限。若同一plugin存在多个符合条件的获批启用版本，返回版本选择冲突并要求显式embedding_routes UUID，不能自动挑latest或最大版本。

查询使用retrieval_query，建立索引使用retrieval_document；前缀由对应适配器按同一已锁定策略生成，不要求这两种用途使用相同文本前缀。text-8的8是实例活动并发上限，只有1vCPU；使用它不自动产生8核或允许无限并行。

## 字段配置固定候选

每字段持有embedding_routes：query_skill_ids与index_skill_ids均为1..8个有序、唯一的不可变Skill UUID，allow_fallback控制同用途候选之间的切换。query与index首选可以不同，但全部候选必须属于同一合法向量空间。

vector-configure启用时的候选来源互斥：

| 方式 | 含义 |
|---|---|
| 显式embedding_routes | 按调用者给定UUID和顺序验证、固定查询/索引候选 |
| 旧式embedding_skill_id | 两种用途使用同一个单候选，allow_fallback=false；不能同时给embedding_routes |
| 首次省略上述两者 | 按部署偏好发现候选，逐个验证后固定为字段路由 |

refresh_routes默认false。已有字段省略embedding_routes/embedding_skill_id且不刷新时保留原候选；refresh_routes=true显式重新按当前部署偏好发现，与显式embedding_routes和旧式embedding_skill_id互斥。失败不提交半份候选，稍后安装的插件也不会在旧字段中自动入列。

自动发现只纳入当前可鉴权、探针成功、模态适配且空间一致的候选；未安装、尚未批准或当前不可验证的预设可以不入选，不要求把全部默认插件安装齐。自动配置以首个验证成功的index优先候选空间为锚点，再按各自优先顺序纳入同空间query/index候选；两个用途均至少得到一个已验证候选才可启用；未验证项不占位等候运行时自动补入。

显式提供的UUID表示精确申请；其中任何候选无法授权、不可探针或空间不符均拒绝整次配置，不静默丢弃调用者显式列出的项。每个入选候选保存Skill UUID/定义摘要、来源installation_id、plugin_id和contract，字段共同绑定实际空间和维度。encoder/投影/前缀等由受信模型锁生成的embedding_space_id覆盖，不能只比对显示名称。

## 探针与空间一致性

ResolveSkill读取固定定义和输入模态，CheckSkillAccess核对当前调用权限、代理审批和就绪状态；随后通过InvokeSkill执行不含业务数据的对应用途探针。ResolveSkill的可读定义不是调用授权，探针成功也不是未来请求的永久授权。

查询与索引候选全部须满足相同embedding_space_id、dimensions、encoder/投影指纹、分词/预处理与前缀策略版本、dtype和截断后L2规则。相同模型名字或相同128维并不足够；向量长度、有限分量和归一化必须实际验证。

文本回退到multimodal-1时，仅允许受审适配器把`items[].text`封装为`items[].parts=[{type:"text",text:原文本}]`，并保持purpose、维度与前缀策略；不得把媒体压成文字或通过外部URL补取。适配后按各profile单批/单项/token上限重新分块调度，同一逻辑索引单元的source_hash和总deadline不变；不得把text profile的16项请求原样送入单项多模态接口。media字段的查询/索引仅纳入支持其实际模态的多模态候选。

## 运行时确定性选择

每次查询或有界reindex单元沿对应已固定列表顺序选择，受当前用户/服务主体、源凭证、插件代理权限及总deadline约束；不能发现新Skill、自动升级安装或跨字段借用另一个空间。fallback串行进行，不向全部候选扇出竞速。

allow_fallback=false只尝试列表首项；true时只有下面可恢复故障才继续下一项。每个固定候选最多尝试一次，全部尝试共享本次根调用剩余期限和重试预算，不能与Broker默认重试叠加。切换前先取消并收敛旧在途embedding，避免超时后同时占用多个备用槽；无法确认旧尝试已退出则按当前取消/超时规则结束，不并行竞速。

| 结果/失败类别 | 路由处理 |
|---|---|
| 候选provider未就绪、不可达、暂时停用或实例退出 | 保留固定候选，allow_fallback=true时尝试下一已验证候选 |
| 明确实例队列满/队列拒绝，且尚未开始该次embedding | 可尝试下一候选；账户日额度/并发额度耗尽不是此类失败 |
| 连接/处理超时，embedding为side_effect=none且仍有原deadline | 可取消旧尝试并串行尝试下一候选；整体deadline已到则直接超时 |
| 当前用户、源链、Skill或代理审批失效，PERMISSION_DENIED/认证错误 | 立即停止，不通过另一个候选绕过撤权 |
| 输入/模态/token限额错误、Blob权限或内容无效 | 立即返回输入/资源错误，不静默截断或借另一预设规避输入限制 |
| 输出维度、embedding_space_id、模型/前缀/encoder指纹不符或无效向量 | 明确兼容性错误，不跨候选掩盖错误或混入索引 |
| 账户/节点额度、存储预算、未知内部错误 | 按原错误处理，不把所有RESOURCE_EXHAUSTED/INTERNAL一概当队列满 |

各预设输入上限不同；备用候选仍必须接受同一逻辑输入并通过自己的输入检查。回退不改变query、purpose、维度、source_hash或根请求幂等关联，不自动把超长文档拆块。

VectorState公开embedding_routes中的query_candidates/index_candidates、allow_fallback和route_digest；每候选记录skill_id、definition_digest、installation_id、plugin_id和contract。旧式诊断字段embedding_skill_id保留首个index候选，不能据此认定查询也用它。

查询vector_stages中的embedding_skill_id和embedding_skill_definition_digest表示本次实际选中的可用query候选；缓存命中或原始vector_query也要先选择获准ready候选，但可以不真正执行模型，因此这些字段不是模型已执行回执。缓存可复用数学结果，命中/返回前仍按当前路由及失败分类完成校验，不能靠缓存跨过撤权或全部provider失效。

## 可用性、原始向量和配置代次

只有当全部允许尝试的query候选均无可用项时，topk才因provider_unavailable暂停；index候选暂不可用只影响新向量建立，不自动删除旧向量。普通行读写、标量match仍可用。

原始vector_query无需再次执行embedding，但仍要求至少一个按路由规则允许的query候选通过CheckSkillAccess且ready。候选定义/空间和原始向量维度仍要匹配；这不是可绕过provider/权限检查的离线模式。

已有固定候选之间发生临时、同空间fallback，不修改generation或source_hash；仍按当前generation/source_hash CAS提交索引，避免将旧行结果写入新行。实际向量内容或状态变化按正常规则增加index_revision并使相关缓存失效。

显式修改/刷新候选集合、顺序、allow_fallback或维度时，vector-configure采用保守统一规则：schema_revision递增、generation递增、index_revision递增，旧向量不再作为当前代次，新代全部重新建立。即使空间相同也不偷偷搬运旧代向量；本版不定义跨代复制/复用迁移。

候选暂时恢复可按原固定UUID继续尝试；新增安装或升级版本只能经显式configure验证后加入。变更失败保持旧配置和旧代次，不能在配置事务提交前半启用新路由。
