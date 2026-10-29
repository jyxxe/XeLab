# 文件元数据微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.6.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-file-metadata/` / `org.xelab.micro-file-metadata` |
| 能力ID | `file.metadata`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；受控文件输入描述接口；MIME类型数据库（按需） |

规范化文件名、扩展名、已声明的大小和MIME类型。首版pure模式只处理调用者提交的元数据描述，结果明确标记为未验证声明。

未来受审核的核心文件metadata适配器可提供已授权且核实过的描述。适配器应在进入VM前完成文件引用授权、大小核实和检测，并通过可信执行封装传入证据；用户正文不能自行声称verified。

能力ID为规划标识；固定制品及MicroDeployment建立后才由核心生成候选Skill。文件引用、所有者、租户和读权限来自核心可信上下文，不能从调用者字段推导。

## 适配器门槛

pure-json-v1没有Blob、文件系统或宿主路径访问。当前规划不读取文件内容、魔数、EXIF或压缩包目录，不给脚本增加Host.ReadBlob。文件名后缀匹配只能给出候选MIME，不能证明内容类型。

MIME数据库需审核并锁定版本。verified模式须等核心metadata适配器的协议、授权证据、字段来源及撤权规则获批后另行发布；未具备适配器时明确拒绝该模式。

## 安全预算

输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。每Call一个描述，文件名最多255 Unicode码点；首版描述大小最多64MiB，扩展名提取不把名字当路径访问。

默认并发1，资源从父Runtime池扣除，无独立容器，不使用内核至少4C16G预留。VM仅虚拟stdin/stdout/stderr，无网络、Secret、进程或原始HostService访问。

大量描述通过[核心异步batch](../../skills/batches.md)处理；各项的声明或验证来源分别保留。
