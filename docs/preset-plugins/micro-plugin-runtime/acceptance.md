# Micro Plugin Runtime验收条件

[MPR目录](README.md) · [JSON执行协议](execution.md) · [安全边界](security.md)

> 规格版本：2.6.0；以下为待实现验收条件，不表示已经执行脚本、浏览器或隔离压测。

| 场景 | 必须成立 |
|---|---|
| 微插件无独立容器 | 多个固定微版本由同一MPR执行，不为每个脚本创建OCI容器；MPR自身仍按普通OCI预设管理 |
| JSON正常路径 | 新VM读取完整输入，输出恰好一个符合固定Schema的JSON值，正常完成后才提交成功 |
| 协议错误 | 空输出、多JSON、调试文字、重复key、非法UTF-8、非有限数值或Schema错误均明确失败 |
| 后续异常 | stdout已写合法JSON后再抛异常/越限，也不能提交成功 |
| 版本与制品 | 代码/Schema变化需新版本；调用固定UUID/digest/微部署，input不能替换source或micro ID |
| 候选产生时机 | 发布版本不生成Skill；创建MicroDeployment才返回candidate_skill，初始disabled，生成不等于注册/启用 |
| 来源审核 | Runtime镜像签名不自动批准用户脚本；核心保存代码审核、owner与不可变内容摘要 |
| 对象权限 | 仅有某微版本执行权不能注册代码、改部署或运行另一版本；父子账户和Key固定上限继续生效 |
| 伪造调用 | 非核心mTLS、错误Runtime安装/代次、过期来源链或自报subject均不能执行 |
| VM隔离 | 前次调用全局变量、输入、输出、凭证和授权上下文不出现在后续或其他用户VM |
| 宿主访问 | filesystem/network/process、普通模块、HostService、secret及Runtime私钥对脚本不可用 |
| 循环与递归 | 无限循环/深递归受独立硬wall与stack限制，能强制销毁执行单元并释放槽 |
| native不可中断 | 模拟引擎native长操作时监督器能强杀worker，不让控制线程与整个实例无限挂起 |
| 内存与输出 | heap16MiB默认/64MiB硬上限、stack1MiB、输入/输出1MiB及深度64得到执行；严格UTF-8、重复key/非有限数被拒绝，输出不截断为伪成功 |
| 日志洪泛 | stderr有界64KiB，超过截断并标stderr_truncated而非业务失败；与stdout分离且按调用权限读取，不吃满内存或阻塞取消 |
| 并发与排队 | 单微部署默认1、硬4；整个实例最多4活动worker、队列32/5s；创建更多微部署不复制父额度 |
| 缓存与淘汰 | 缓存256MiB/128制品/TTL300s先触边界即过期或LRU淘汰；新VM不共享可变状态；淘汰不删除核心版本/结果 |
| 取消与迟到 | Cancel只命中准确执行单元，迟到stdout不覆盖终态；其他并发微调用不被误杀 |
| Runtime故障 | 相关微部署waiting_runtime，不自动切换安装；核心管理API、CLI及其他插件仍可用 |
| 重启与查询 | MPR缓存可丢弃，核心Call仍可查询；未确认执行按原固定版本核实/恢复，不假称已成功 |
| 异步batch | 每item遵守同样权限与执行上限；202仅表示接受，逐项状态/结果与核心batch协议一致 |
| 资源边界 | worker地址空间128MiB不是RSS；sandbox512MiB+控制128MiB+缓存256MiB合计896MiB，Initialize验证不超过父1GiB；无cgroup委派或逐worker独立CPU/RSS隔离声明 |
| 总期限 | 默认2000ms/硬30000ms覆盖调用所需Prepare、排队、编译和执行，各阶段不能重置原deadline |

文档工具可验证清单、Schema、样例与接口一致性；真实JS引擎限制、跨VM隔离、worker强杀、内存峰值与恢复必须在运行时实现后单独验证。
