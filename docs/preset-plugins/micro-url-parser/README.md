# URL解析微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-url-parser/` / `org.xelab.micro-url-parser` |
| 能力ID | `url.parse`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；URL解析库或经审核的运行时内置解析器 |

将绝对URL分解为协议、主机、端口、路径、查询参数和片段，用于数据清洗及后续业务判断。该能力不执行访问、DNS查询、重定向跟随或文件读取。

能力ID为规划索引标识。未来MicroDeployment创建后才由核心生成候选Skill，发布微版本本身不注册可调用的核心能力。

## 解析门槛

必须固定解析器版本及URL规范化规则，覆盖Unicode域名、IPv6、百分号编码、重复参数和默认端口测试。当前Runtime不保证存在浏览器URL全局对象，不能未经审核依赖宿主解析器。

首版只接收绝对HTTP/HTTPS URL并拒绝用户信息段，减少凭证被复制到输出和日志。其他scheme、相对URL和base拼接留给另行版本定义。

解析成功只证明符合本插件的语法。网络访问仍由实际HTTP或浏览器能力按当前身份、目标和egress规则检查；解析结果不能用作绕过SSRF检查的凭证。

## 安全预算

输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。单URL最多8192 Unicode码点，查询参数最多1000对；总UTF-8字节预算仍独立生效。

默认并发1，资源从父Runtime池扣除，无独立容器，不占用内核至少4C16G预留。仅虚拟stdin/stdout/stderr，无文件、网络、Secret、进程或原始HostService访问。

多URL通过[核心异步batch](../../skills/batches.md)提交，逐项查询结果；不在单Call内进行不受限批量解析。
