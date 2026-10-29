# 受限模板微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-flow-template/` / `org.xelab.micro-flow-template` |
| 能力ID | `flow.template`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；受限模板引擎 |

以调用者提供的模板和JSON数据生成文本，支持固定的字段占位符及声明式过滤器。需要结构化内容时，调用者可选择对完整渲染文本进行一次JSON解析和校验。

能力ID为规划标识。未来发布固定制品、核验摘要并创建MicroDeployment后，由核心生成候选Skill；完成授权和启用后才能调用。

## 引擎门槛

只规划字段替换、固定过滤器与字面量转义。模板不读取文件、不include/import、不递归执行模板、不调用函数或任意宿主代码；数据字符串不会被二次解释为模板。

模板解析器、转义实现和完整JSON输出校验需随不可变微版本审核并通过Runtime门槛。尚无已安装模板库的保证；增加循环、扩展过滤器或新语法需新版本及新的资源分析。

生成的HTML、SQL或命令文本仍须由使用方按目标上下文处理；通用模板输出不自动授予执行、网络或数据写入权限。日志只记录有界错误位置，不默认记录模板数据。

## 安全预算

每Call输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。模板正文最多262144 Unicode码点，占位符最多4096个；完整输入字节及输出预算仍独立生效，越限整次失败。

微部署默认并发1，资源归父Runtime管理，不创建独立资源池，不占用内核至少4C16G预留。VM只有虚拟stdin/stdout/stderr，无文件、网络、Secret、进程和原始HostService访问。

批量渲染使用[核心异步batch](../../skills/batches.md)，按item查询结果；每次渲染遵守相同预算。
