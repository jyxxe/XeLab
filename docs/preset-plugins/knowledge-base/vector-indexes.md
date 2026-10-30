# 按字段向量开关与128维默认

[知识库目录](README.md) · [查询/索引路由](embedding-routing.md) · [索引一致性](index-consistency.md)

> 规格版本：2.8.0；状态：设计契约，尚未实现。

## 支持范围和维度

仅string/text/media列支持向量，默认enabled=false。每字段独立配置维度、query/index候选和generation，不把整行或全部媒体自动合并成一条向量。

首次启用dimensions默认128，可明确选128/256/512/768；后续省略dimensions保留当前值。数据按little-endian float32存储，128维原始向量每条512字节，另有版本/哈希/数据库开销。

关闭字段保留已有维度/空间用于核查。原始查询向量必须长度与实际空间完全匹配，检查有限值和非零后L2归一化；不能把768维输入误截为128维。

## 文本查询与建立索引分别路由

文本query默认优先org.xelab.embedding-gemma.text-1，index默认优先org.xelab.embedding-gemma.text-8；默认可回退到其他已验证同空间候选。media字段只使用支持其模态的多模态候选。完整配置、失败分类和fallback顺序见[embedding路由](embedding-routing.md)。

每字段embedding_routes固定有序query_skill_ids、index_skill_ids和allow_fallback。候选按UUID、定义digest及installation_id绑定，运行时不按latest或插件名重新发现。

启用时可显式给embedding_routes，或用旧式embedding_skill_id表示两用途同一单候选且不回退；两者互斥。首次均省略则按部署embedding_routing偏好发现当前可验证项，缺少其他可选预设不强迫失败，但每用途至少一个候选。

后续省略候选且refresh_routes=false保留原绑定。refresh_routes=true才重新按偏好发现，与两个显式候选字段互斥；发现使用ListSkillCandidates，多个符合条件的同插件版本必须由调用者显式选UUID，不按latest解决冲突。

## 配置与探针

vector-configure按当前主体授权，以expected_schema_revision进行CAS。ResolveSkill读取定义/模态，CheckSkillAccess验证调用权、代理审批与ready，InvokeSkill对每个入选候选执行受控无业务数据探针，确认实际维度、空间及数值有效性。

query/index的首选可以不同，但全部候选必须匹配同一embedding_space_id、维度、encoder/投影、前缀策略及预处理指纹；相同模型名或维度不是兼容证明。查询使用retrieval_query，建立索引用retrieval_document，不能把“同前缀策略”理解为两用途必须使用相同输入前缀。

VectorState.embedding_routes公开两用途固定候选及route_digest；兼容诊断字段embedding_skill_id只代表首个index候选。每次topk元数据中的embedding_skill_id才代表该查询选中的候选，缓存/原始向量路径可以只鉴权而不执行模型。

只有当前可鉴权且验证成功的发现候选才能入选；无法验证的预设不会在运行时自行加入。显式候选中任何一项不合规使整次配置失败。验证、schema CAS及配置提交整体成功后才启用；失败保留原配置，不产生半启用字段。

## 代次、重建与运行回退

显式改变或刷新候选列表、顺序、allow_fallback、维度等字段向量配置时，schema_revision、generation和index_revision递增；旧代向量停止参与查询，新代pending并重新建立。空间相同也按本版统一规则重建，不隐式把旧向量挂到新代。

运行时在已固定候选中因明确可恢复故障进行同空间fallback，不改generation或source_hash；只切换本次实际执行的Skill，仍共享原deadline、当前授权和实例资源限制。索引提交继续检查generation/source_hash，模型返回后行变化或配置变化不得提交旧结果。

禁用字段不删除表值或媒体，也不自动删除历史向量；vector-drop要求先禁用并明确确认，才清理派生向量。删除列不是隐式释放全部业务数据的替代接口。

## provider与权限失效

没有可验证embedding候选时不允许新启用向量，普通表/行/标量match继续可用。已有字段仅当所有按路由规则可尝试的query候选均无可用项时暂停topk；一个候选临时不可用而有获准备用项时不必停掉整字段查询。

权限、源链、输入或空间错误不得作为可跳过失败；不能以备用provider规避撤权。index候选暂时全部不可用会暂停新向量建立并保留pending/已有向量，不要求查询候选也一并失效。

即使调用者提供原始向量，仍至少需要一个query候选通过当前CheckSkillAccess且ready；可跳过实际embedding，但不能用原始向量或缓存宣称脱离provider的离线服务模式。候选恢复沿原固定版本继续，换安装/模型需重新配置并重建。

## 索引内容修订号

generation标识字段配置代次，index_revision标识可见索引内容/状态。相同代次内reindex继续完成或向量有效性变化也要增加index_revision；运行fallback本身不增加代次，成功写入产生的可见变化仍照常推进修订号。

查询各topk阶段与[缓存](query-cache.md)绑定实际generation/index_revision/space及有效路由信息，不能仅凭data_revision或一个首选provider状态复用旧结果。详细并发规则见[索引一致性](index-consistency.md)。
