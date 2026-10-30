# 摘要计算微插件

[预设目录](../README.md) · [接口草案](interfaces.md) · [微插件架构](../../architecture/micro-plugins.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。以下内容为待实现规划。

| 项目 | 规划 |
|---|---|
| 目录 / 预设ID | `micro-hash/` / `org.xelab.micro-hash` |
| 能力ID | `crypto.hash`，规划索引标识 |
| 执行方式 | Micro Plugin Runtime共享受限VM，无独立容器 |
| 执行协议 | `javascript`、`json-stdio-v1`、`pure-json-v1`、`side_effect=none` |
| 依赖 | `contracts/`；`micro-plugin-runtime/`；审核过的密码学哈希库或运行时内置实现 |

对有界文本或base64表示的二进制数据计算SHA-256摘要，用于内容去重和完整性比较。摘要不证明数据来源、作者身份或访问授权，不将普通哈希包装成签名能力。

能力ID为规划索引。未来经固定制品发布、摘要核验和MicroDeployment创建，由核心生成候选Skill；调用输入不能选择任意宿主库、可执行命令或运行时。

## 实现与文件门槛

哈希实现须固定版本，通过标准测试向量、空输入和分块一致性检查，并满足VM预算。当前Runtime不保证存在WebCrypto、Node crypto或可导入的系统库；未过库门槛时保持planned。

pure-json-v1只接收正文数据，无Blob读取、文件系统、网络或原始HostService。不得增加Host.ReadBlob或把host_path当输入来绕过协议边界。

整文件流式摘要需未来受控流输入协议、对象版本绑定、读权限与撤权检查、字节上限及取消规则获批。1MiB JSON封装限制下，base64数据还要承担约4/3编码开销，不能声称支持任意大文件。

## 安全预算

每Call输入/输出各最多1MiB UTF-8 JSON、深度64；heap默认16MiB、stack 1MiB；wall默认2s、平台硬上限30s。算法首版仅sha256，输入解码与摘要处理均计入期限，不截断数据后返回成功摘要。

默认并发1，资源从父Runtime池扣除，不独立创建容器，不使用内核至少4C/16GiB预留。虚拟I/O仅stdin/stdout/stderr，日志不默认保存原文或解码后的字节。

多条有界数据通过[核心异步batch](../../skills/batches.md)计算，逐项查询结果；不同item的摘要不能自动合成为原文件摘要。

## 微部署设置

依赖MPR 1.2.0。每微部署默认 `resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB）；默认活动并发1、硬上限4，同一微部署的所有worker合计共享CPU/内存硬上限，JS heap 16MiB仍是每VM的子上限。核心适配器配置逐微cgroup，不创建每微OCI，也不向MPR授予宿主cgroup管理权。

`network_enabled`默认false，不影响本插件的Prepare/Ready和纯计算调用。本插件固定pure-json-v1，开关不能增加HTTP桥、fetch/socket或任何宿主能力；受控HTTP还须固定profile、权限、HttpBinding及平台策略共同批准。

资源与联网设置可手动配置：先停用微部署并等待phase=disabled且完全排空，再通过[设置接口](../planned/interface-rules.md#微部署资源与联网设置)更新，然后重新启用。更新generation/revision并使旧句柄失效，不修改原limits、微版本或父绑定，不改变已注册Skill定义。

父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB；默认预算最多允许4个不同微部署同时活跃，总活动槽4只是上限。资源从父预算扣除，不借用内核4C/16GiB预留。
