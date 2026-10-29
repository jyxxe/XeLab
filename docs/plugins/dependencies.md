# 依赖解析与绑定

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 输入与结果

输入为 [DependencySpec](../api/models/DependencySpec.md)，输出为 [DependencyPlan](../api/models/DependencyPlan.md)。required、target_kind、binding_scope 是独立维度；计划只是预览，执行前重新核对安装、授权与资源版本。

解析顺序：匹配能力/实现 → 精确契约和 digest → features → 当前权限/数据范围 → 健康 → 固定优先级 → deployment_id 排序。相同输入和状态必须得到相同结果；首版不使用随机成本策略。

强依赖图必须无环；首版拒绝所有强依赖环，不保留“也许可初始化”的隐式例外。可选依赖不阻塞插件启动，但相关能力必须降级。

## 绑定范围

| 范围 | 生命周期 | 切换条件 |
|---|---|---|
| request | 单次调用 | 仅未提交、可安全重试且语义兼容时 |
| deployment | 部署代次 | 管理变更或明确降级/重绑；配置 revision 递增 |
| task | 整个任务 | 任务内保存提供者，不因负载变化改绑；恢复需确认数据一致性 |

契约兼容不能证明索引、会话或业务状态共享。创建索引后读取必须固定到拥有索引的提供者或共享数据域。

多个消费者共享提供者只计算一次实例资源；消费者引用数用于依赖管理，不是副本数。禁用被引用的强依赖部署要明确提示影响并按依赖策略暂停消费者。

HostService.Resolve 返回不透明 binding_id，不返回实例内网地址。绑定不是授权：每次 Invoke 仍检查当前主体、源链和插件代理权限。

## 微插件依赖边

微版本固定runtime_installation_id，MicroDeployment固定普通runtime_deployment_id；内核检查Manifest.micro_runtime及engine/protocol，不将它混入普通capability依赖。runtime离线使微部署waiting_runtime，不阻塞核心就绪；升级不能把旧微版本自动改绑新OCI安装，详见[微插件控制器](../architecture/micro-plugins.md)。
