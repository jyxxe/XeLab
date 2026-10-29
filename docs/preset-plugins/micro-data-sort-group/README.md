# 记录排序与分组微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-data-sort-group/` / `org.xelab.micro-data-sort-group` |
| 能力ID | `data.sort`、`data.group`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；有界结构化数据处理能力 |

对调用者已提交的有界记录数组执行稳定多键排序或按键分组，返回完整结果与原始下标。此能力不访问数据库，不调用知识库查询，也不借排序表达式执行任意代码。

能力ID只是规划索引。未来固定制品发布和MicroDeployment创建后，由核心按微部署生成候选Skill；两个业务入口必须各自绑定固定的输入输出契约。

## 算法门槛

键通过JSON Pointer读取，只允许同类型有限数值、字符串、布尔或null，比较顺序及空值策略在接口中固定。禁止自定义比较器、eval、动态属性执行和locale依赖的隐式排序。

稳定排序、分组键编码和内存监督须使用已审核实现并固定版本；可采用受限JavaScript实现，无需宿主文件或网络。不能为节省内存丢掉记录、自动取topk或静默截断组。

组以数组输出，不把用户键直接写成动态JSON对象属性。限制组数与中间内存，防止高基数输入突破共享Runtime资源。

## 安全预算

输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。每Call最多10000记录、8个键；max_groups最多10000，输出与中间索引均计入预算。

微部署默认并发1，资源从父Runtime池扣除，无独立容器，不使用内核至少4C16G预留。仅虚拟stdin/stdout/stderr，无文件、网络、Secret、进程或原始HostService。

多份独立数组走[核心异步batch](../../skills/batches.md)。分片排序结果不能直接当作全局有序结果，跨item聚合需显式上层流程。
