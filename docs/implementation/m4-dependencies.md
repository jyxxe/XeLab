# M4 第一阶段：依赖解析与固定路由

[实施索引](README.md) · [目标 #51](https://github.com/jyxxe/XeLab/issues/51) ·
[阶段 #52](https://github.com/jyxxe/XeLab/issues/52) · [规范](../plugins/dependencies.md)

## 可用行为

`resolveDependencies` 为真实管理接口，读取当前安装、部署、签名契约、健康代次、
特性和当前发现权限，按部署 UUID、能力名、契约版本及实例 UUID 稳定排序。
当前契约没有提供者优先级字段，所有提供者优先级相同，不引入随机或 latest 策略。
插件依赖匹配插件版本；能力依赖匹配能力契约版本，两者不能相互推导。

插件依赖可选择不提供 capability 的健康普通插件。此时 `DependencyBinding.contract`
为 null；依赖仍固定精确部署、安装、清单摘要、配置和运行代次，不制造假能力契约。
能力依赖返回精确 `name/version/schema_digest`。特性来自通过工作负载身份校验的
健康响应；旧插件没有新增 `CapabilityHealth.features` 时按空列表处理。

创建部署不自动启用或分配提供者副本。启用事务重新解析并提交全部固定绑定，
遇到缺失强依赖、强环、超过 16 层或图规模上限时整份拒绝。
授权事务、Call 账本和依赖图锁按统一顺序串行化并发图变更。
计划只作预览，不能作为后续执行的授权票据。多个消费者引用同一提供者的增量资源为 0；
消费者自身的实例资源仍由原准入流程单独预留。

## 范围和失效策略

| 范围/策略 | 实际处理 |
|---|---|
| deployment | 精确提供者及配置/代次固定；禁用再启用是显式重新解析 |
| request | 每个真实父 Call 独立解析，返回有期限、不含地址的不透明句柄；同一 Call 重复解析复用句柄 |
| task | 解析、图检查和元数据保留；没有 TaskEngine 时拒绝任务执行范围，不能假造任务身份 |
| wait | 固定范围内提供者失效后暂停，原提供者以原配置/代次恢复才可继续；不替换已提交调用 |
| fail | 部署范围故障持久锁定，恢复提供者仍需显式重新启用；请求范围结束当前固定调用 |
| fallback | 仅新请求尚未提交时按同一精确契约选择健康提供者；已有句柄及部署范围不自动切换 |
| degrade | 可选依赖缺失不阻塞基础启动，公开 `dependency_degraded:<alias>` 条件，相关 Resolve 拒绝；强依赖仍阻塞 |

可选 request 依赖在启用时不存在，之后出现提供者时，可由后续真实父 Call 解析，
无需重新启用消费者。实际解析受该次调用者、父委托完整来源链及插件代理审批交集约束。
路由绑定不是执行委托，创建部署时的交互会话过期不使静态依赖自动失效。
撤销实际执行来源后，后续发现、调用和返回数据仍须由 Host/Broker 拒绝。

强依赖的禁用会先撤销消费者的可见路由和当前委托；控制器随后物理停止消费者，
有未知副作用的 Call 保留 unknown，不自动重放。物理停止前不释放资源预留。
仍启用且等待固定强依赖的消费者保留自身原预留；显式停用并清理后才释放，
等待期间不为共享提供者重复计费或新增预留。
健康、特性、安装、配置或代次漂移使旧句柄失效，不能借新安装继续使用旧绑定。

升级目标依赖在接收操作时保存精确计划，后续工作与发布前重验原操作授权和固定目标。
最终发布在同一图事务替换绑定；失败不改旧源绑定，也不在恢复时静默发现另一提供者。
恢复后的旧请求句柄受原父 Call 和当前委托检查，不能作为跨进程持久授权。

## 兼容性与验证

Schema 17 为追加迁移；不修改既有 SQL 指纹、签名清单、镜像或旧权限快照。
`DependencyBinding.contract` 扩展为可空以表达真实插件依赖；能力绑定仍要求完整引用。
`CapabilityHealth.features = 4` 为可选新增 wire 字段，数量、长度、唯一性由核心验证。
微插件/MPR、扩展图类型仍保持现有拒绝边界；未交付的任务系统不在本阶段实现。

针对性命令：

```bash
cargo test --lib dependencies::tests --locked -- --nocapture --test-threads=1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 docs/tools/validate_docs.py --strict
```

PostgreSQL 测试需要 `XELAB_TEST_DATABASE_URL`；缺少时会明确输出 SKIPPED，不能记为已执行。
覆盖并发强环、深度、确定性、插件/能力版本、可选依赖、固定配置/代次/特性、fail 锁定、
实际管理接口幂等、请求句柄隔离和撤销当前调用凭证。正式宿主生产验收仍由
[#50](https://github.com/jyxxe/XeLab/issues/50) 独立跟踪；AWS 继续 DEFERRED。
