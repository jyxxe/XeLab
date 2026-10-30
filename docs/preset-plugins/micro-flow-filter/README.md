# 数组条件过滤微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-flow-filter/` / `org.xelab.micro-flow-filter` |
| 能力ID | `flow.filter`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享的受限VM；不创建独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；与[条件微插件](../micro-flow-condition/README.md)共享的声明式条件解析器 |

对有界JSON数组逐项求值，按原顺序返回满足条件的元素，适用于工作流中的记录筛选。条件引用当前元素的JSON Pointer，不遍历其他用户的数据或调用其他Skill。

能力ID不构成已注册Runtime能力。未来经版本发布、摘要核验与MicroDeployment创建后，核心才生成绑定该微部署的候选Skill，审批与启用遵循[微插件生命周期](../../architecture/micro-plugins.md)。

## 语义与依赖门槛

共享条件AST使用固定操作集合、严格类型和有界递归；不接受JavaScript、SQL或知识库查询语言。字段缺失和显式null分别处理。输出保留重复元素及原顺序，不能因输出预算不足静默丢弃匹配项。

AST解析器、JSON Pointer解析器与求值步数监督必须经过Runtime审核并固定版本后才能发布。首版可用受限JavaScript实现，无需额外宿主权限；库尚未预装或验收时保持planned。

## 预算与批量

每Call输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。更低的Schema及微部署限额优先；AST深度16、节点256，输入最多10000项。

单微部署默认并发1，受父Runtime总活动槽约束。所有预算从父Runtime资源池扣除，不重复申请容器资源，不占用内核至少4C/16GiB预留。

虚拟I/O仅允许stdin/stdout/stderr；无文件、网络、Secret、进程或原始HostService能力。大批量使用[核心异步batch](../../skills/batches.md)，逐项查询状态和结果；单项限额保持不变。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
