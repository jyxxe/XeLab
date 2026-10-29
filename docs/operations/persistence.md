# 元数据库、约束与事务边界

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 结构来源

[metadata.sql](../contracts/metadata.sql) 为 PostgreSQL 逻辑基线，尚未应用到数据库。HTTP 模型与表不一一对应：响应是脱敏投影，普通业务输入输出只能进入相应 Task/Blob 等受控记录，插件业务数据库不能直接复用平台元库。

## 核心关系

- accounts/account_closure/inheritance_grants 保存账户树；credentials 保存源凭证链；grant_provenance 保存快照的当前授权来源，防止解绑后留下永久 allow。
- groups/group_versions/grants 区分定义、不可变版本与实际绑定；roles 仅内置模板。
- installations/deployments/instances 分离制品、期望与观测；reservations 对应实际占额。
- provider_records 与 dependency_bindings 是可重建路由/绑定，不是授权凭证。
- tasks/task_steps/attempts 和 execution_delegations 共同决定可执行性；lease/fence 不能只存在内存。
- events/outbox/subscriptions/deliveries 管理持久投递和确认，dead_letter 终态仍保留记录。
- audit_events 追加，业务数据库用户不授予 UPDATE/DELETE 审计表权限。

## 必须由事务保证的约束

SQL 基线提供类型、FK、唯一键及基础 CHECK；以下跨行约束必须由内核事务/触发器实现，并在实现验收中测试：

| 操作 | 锁及原子条件 |
|---|---|
| 建子账户/签发 Key | 根至叶锁祖先，完整可授范围、最大深度、树额度与 source chain 无环 |
| 禁用 Owner | 锁独立 Owner 集合，至少一名有效恢复根留下 |
| 绑组/调整继承 | 当前源授权版本、目标版本和来源记录同事务写入，失效通知同 outbox |
| 实例准入 | node_budget + instance 唯一 reservation，不超过插件池 |
| 单写卷挂载 | 锁 volume，确认旧 writer 真实退出，fence 单调增长 |
| Task claim/commit | 当前步骤/租约/fence/授权，单活动 attempt 唯一索引 |
| Event ack | 当前 subscription/lease；旧 lease 不能推进新游标 |
| 幂等提交 | 记录指纹、资源变更、Operation 和安全响应同事务 |

## 迁移与存储保留

数据库迁移按递增版本和 checksum 执行，禁止运行时自动执行破坏性迁移。升级前备份元库及卷的一致性点，DDL 回退与业务数据回退分开。

不使用 ON DELETE CASCADE 清理授权链或业务历史。GC 按引用和保留策略显式执行；操作、已引用检查点、回滚点和未确认投递均需 pin，不能只看创建时间删除。

## 批次与UI偏好

skill_batches/items保存固定输入、执行委托、稳定逐项业务键、租约/fence、计数和私有结果引用。元数据与outbox原子提交；大结果先写不可变对象，引用失败成为GC对象。输入/结果独立累计账户树批次存储预算，保留和恢复详见[批次执行](../tasks/batches.md)。ui_preferences按owner唯一保存非敏感主题/布局，贡献直接来自签名Manifest.ui，不另设可变插件自报注册表。

## 核心微制品与运行绑定

micro_plugins/versions/deployments由核心PostgreSQL管理，源码先复制私有不可变对象存储再原子提交包摘要/版本/幂等/outbox，失败孤儿由GC清理。MPR无需持久卷；micro_runtime_bindings仅缓存实例/微代次及prepared_handle，重启后可重建，不将handle当授权票据。skills.source_kind区分OCI签名来源与微制品来源，calls和诊断保存微版本/包摘要。

源码及清单计入独立账户树微制品存储预算，初始1GiB；初始每账户树100个未归档微插件、每插件20个保留版本、100个未归档微部署，按根至叶锁定计数，超限QUOTA_CONFLICT，不因建子账户复制预算。这些是初始策略，修改需配置/验收，不是运行时额外资源预留。

归档先撤路由保留历史，不立即删除源码；归档30天后且无未完成Task/Batch、活动Skill或运行绑定引用才允许回收制品，保留摘要审计。微诊断最多7天且遵守磁盘水位，可提前淘汰返回410，不影响已持久业务结果；不把私有stderr复制到普通运行时日志。

## 微HTTP与对象文件账本

micro_http_bindings保存固定微部署别名/Secret绑定与优先级；每Call以micro_http_plans冻结有序配置和Secret版本，micro_http_operations在出网前写seq/alias唯一记录。相同指纹只重放已知结果，已发不明为unknown并阻止后续HTTP，不能靠脚本catch改为成功。优先级/超时变化不改旧快照，当前禁用/删除/撤权仍生效；换Secret或轮换版本不让旧Call偷换凭证。已完成受控响应私有保存最多7天，unknown核实记录最多30天，默认审计不保存query/密钥/原始错误body。

对象存储的数据、永久key head/tombstone、内容文件及幂等记录属于插件专属卷，见[私有SQL](../preset-plugins/object-storage/storage.sql)。备份必须保留head修订水位和所有被引用内容，不能仅备份核心Blob或只备份SQLite。
