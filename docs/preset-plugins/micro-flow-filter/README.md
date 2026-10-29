# 数组条件过滤微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

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

单微部署默认并发1，受父Runtime总活动槽约束。所有预算从父Runtime资源池扣除，不重复申请容器资源，不占用内核至少4C16G预留。

虚拟I/O仅允许stdin/stdout/stderr；无文件、网络、Secret、进程或原始HostService能力。大批量使用[核心异步batch](../../skills/batches.md)，逐项查询状态和结果；单项限额保持不变。
