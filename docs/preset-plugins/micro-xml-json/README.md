# XML与JSON转换微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-xml-json/` / `org.xelab.micro-xml-json` |
| 能力ID | `convert.xml-json`，双向转换共用的规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；受限XML解析/序列化库；JSON处理能力 |

通过direction区分XML到JSON树和JSON树到XML，显式保留元素顺序、重复子节点、属性、命名空间和混合文本。首版不采用把重复节点临时改为数组的含糊映射。

能力ID只用于规划索引。固定制品通过审核并创建MicroDeployment后，由核心生成候选Skill；两个方向不另注册未经规划的能力ID。

## XML库门槛

解析器须能够强制禁用DTD、外部实体、自定义实体扩展、外部Schema加载、XInclude和任何联网行为，并在VM外层预算以内限制树构造。配置不支持这些限制的库不能启用。

只允许预定义XML实体和合法数字字符引用，替换为对应文本不等于允许一般实体扩展。禁用XML处理指令及可执行扩展；XML声明仅可声明版本1.0和UTF-8。

库、映射和序列化测试需锁定版本；当前MPR没有已安装XML解析库的承诺。注释、CDATA边界、前缀选择和属性引号形式不保证字节级还原，结构化映射规则见接口。

## 安全预算

输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。XML树最多10000元素、每元素128属性，XML实际嵌套还受更低的输出JSON深度预算约束。

默认并发1，资源从父Runtime池扣除，无独立容器，不使用内核至少4C/16GiB预留。仅虚拟stdin/stdout/stderr，无文件、网络、Secret、进程或原始HostService访问。

多文档使用[核心异步batch](../../skills/batches.md)，超限或畸形XML整次失败，不输出不完整树。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
