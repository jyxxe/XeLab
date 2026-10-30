# 数据脱敏微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-data-redactor/` / `org.xelab.micro-data-redactor` |
| 能力ID | `data.redact`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；脱敏规则集；受限正则引擎（启用正则规则时） |

按明确JSON Pointer定位字段，执行替换、遮罩或受限模式替换，并返回处理后的JSON副本及有界统计。适合出站消息、日志采集前处理和数据展示中的字段保护。

能力ID只用于规划索引；固定制品发布及MicroDeployment创建后，核心才生成候选Skill，调用权限遵循微部署授权。

## 规则与库门槛

规则按声明顺序作用于前一步结果。默认不保存原始值，不在诊断中返回命中内容；日志策略不能抵消调用方主动保留输入所带来的风险。

字段替换和遮罩可用有界纯JSON处理实现。正则规则依赖经过审核、禁止灾难性回溯的引擎及版本；Runtime未提供该引擎时拒绝启用正则规则，不能退回不受限宿主RegExp。

脱敏结果取决于字段选择、规则质量和使用场景。规则命中数不证明完整去标识，也不构成绝对匿名的保证。添加内置敏感信息检测规则集必须记录版本并给出覆盖范围。

## 安全预算

输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。最多128条规则，每条模式最多1024字符；求值和替换总次数有界。

微部署默认并发1，资源从父Runtime池扣除，不独立创建容器，不使用内核至少4C/16GiB预留。VM只有虚拟stdin/stdout/stderr，无文件、网络、Secret、进程或原始HostService能力。

大量独立数据脱敏使用[核心异步batch](../../skills/batches.md)，按授权读取逐项结果。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
