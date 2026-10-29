# 数据脱敏微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

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

微部署默认并发1，资源从父Runtime池扣除，不独立创建容器，不使用内核至少4C16G预留。VM只有虚拟stdin/stdout/stderr，无文件、网络、Secret、进程或原始HostService能力。

大量独立数据脱敏使用[核心异步batch](../../skills/batches.md)，按授权读取逐项结果。
