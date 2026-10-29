# Micro Plugin Runtime：微插件运行时

[预设目录](../README.md) · [核心Skill层](../../skills/README.md) · [文档总目录](../../README.md)

> 规格版本：2.6.0；状态：待实现的设计预设，尚无运行时、镜像或隔离压测结果。

插件ID为`org.xelab.micro-plugin-runtime`，版本`1.0.0`。启用MPR后，可把一个读取JSON并输出JSON的轻量脚本注册为微插件。每个微插件无需制作OCI镜像或启动自己的容器，多个微插件由同一个MPR实例按限额执行。

MPR本身仍是普通OCI预设，使用现有安装、审批、资源池和生命周期。当前定义的是微插件无独立容器；完全不使用OCI的宿主直接执行模式另需运行时与安全契约。

## 运行路径

```mermaid
flowchart LR
    U[用户 / WebUI / batch] --> S[核心固定版本Skill]
    S --> A[微对象权限与制品核验]
    A --> R[MPR受控RPC]
    R --> W[短命隔离执行单元]
    W --> J[完整JSON结果]
```

微插件、版本和部署是核心管理的第一类对象，按真实资源归属、不可变制品和当前权限管理。发布微版本本身不生成Skill；创建MicroDeployment后才得到其candidate_skill，初始enabled=false、phase=disabled，不自动注册或启用Skill。

候选注册后使用`target.kind=micro`，固定微版本、package_digest和micro_deployment_id。微部署enabled和Skill.enabled分别管理，只有父Runtime当前获批/Ready且Prepare成功才可调用；用户输入不能更换脚本。

MPR不向用户发布可随意指定脚本的通用execute Skill，不负责替用户注册或审批微对象。脚本来源由核心记录的owner审核与内容摘要固定，不能把它当作MPR镜像签名已覆盖的代码。

## 初始建议

| 项目 | 默认建议 |
|---|---|
| 运行时资源 | 1000mCPU / 1GiB内存 / 256MiB临时盘 / pids128 |
| 实例 | 单副本，无持久卷，data_schema_version=none |
| 业务并发 | 整个MPR活动4、队列32、排队最多5s |
| 制品缓存 | 内存256MiB，最多128份，TTL300s并按LRU淘汰 |
| 单次微调用 | 总期限2000ms、JS heap 16MiB、stack 1MiB |
| 单微部署并发 | 默认1、硬上限4，再受父MPR总4个活动槽约束 |
| JSON与日志 | 输入/输出各最多1MiB、深度64，日志最多64KiB，超限截断并标记 |

这些是待测初始配置，计入原插件池，不增加或借用内核至少4C16G的预留。并发限额作用于明确微部署和父MPR实例，不为MicroPlugin跨不同父实例凭空声明一个独立全局配额。

## 文档

- [配置与限额](configuration.md)
- [JSON执行协议](execution.md)
- [权限与隔离](security.md)
- [接口与Skill绑定](interfaces.md)
- [部署与恢复](deployment.md)
- [验收条件](acceptance.md)

第一版engine为`javascript`，协议为`json-stdio-v1`。`stdin`、`stdout`、`stderr`是受控虚拟缓冲；只暴露明确的JSON处理能力，不提供普通Node/Python模块或脚本访问HostService的入口。

[JSON脚本及完整注册/调用样例](examples/README.md) · [动态Skill说明](../../skills/micro-plugin-runtime/README.md) · [逐核心HTTP接口](../../api/operations/micro-plugins/README.md)。
