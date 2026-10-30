# 文件元数据微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

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

默认并发1，资源从父Runtime池扣除，无独立容器，不使用内核至少4C/16GiB预留。VM仅虚拟stdin/stdout/stderr，无网络、Secret、进程或原始HostService访问。

大量描述通过[核心异步batch](../../skills/batches.md)处理；各项的声明或验证来源分别保留。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
