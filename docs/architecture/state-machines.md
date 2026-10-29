# 状态机与转换前置条件

[文档总目录](../README.md)

> 规格版本：2.6.0；状态：待实现的规范性设计。MUST/必须表示实现及验收要求，不能理解为现有功能。

## 部署与实例

| 对象 | 来源 → 目标 | 必需条件 / 失败处理 |
|---|---|---|
| Deployment | disabled → reconciling | enabled=true、安装/审批有效；失败留待重试或 failed |
| Deployment | reconciling → waiting_resources | 插件池不足；不创建未记账实例 |
| Instance | pending → starting | 预留已提交，稳定实例 ID 已落库 |
| Instance | starting → ready | Initialize 成功，代次一致，必需依赖及该能力就绪 |
| Instance | ready → degraded | 部分能力不可用；只撤销受影响能力 |
| Instance | ready/degraded → draining | 禁用、升级或健康失败，先撤销新流量 |
| Instance | draining → stopped | 排空结束或到期停止；观测容器已退出后回收运行预留 |
| Instance | 任意非终态 → failed | 启动失败、OOM、探针失效；记录原因，有限重启 |

部署 phase 为聚合观测，不是 desired enabled 的替代。revision 是期望变更代次，observed_generation 只有控制器可推进。状态转换与 operation 进度通过事务记录。

## 管理操作

pending → running → succeeded/failed；取消为 pending/running → cancel_requested → canceled。已越过不可逆提交点时可能继续 succeeded，不能把取消请求当作副作用撤回证明。

## 账户与凭证

pending → active 需要一次性激活；active ↔ disabled 需要授权；active/disabled → archived 保留授权链。祖先 disabled 会使下级有效性失效，但不会删除下级记录。

credential active → revoked/expired 为单向终态；恢复祖先不能复活已 revoked 的凭证。单个登录会话退出不等于账户禁用。

## 任务

queued → running → succeeded/failed/canceled；依赖不可用进入 waiting_dependency；授权失效进入 paused_authorization。执行结果无法确认进入 unknown，必须查询或核实，不能直接转换为 failed 后自动重试。

所有转换条件须在服务端执行，HTTP 只允许显式定义的期望状态变更，禁止 PATCH 任意 state 字段。

## 微插件与微部署

MicroPlugin的归档是核心期望变更，撤销其新版本发布/新路由并保留历史资源；MicroPluginVersion发布后内容不可变，不通过状态切换原地替换源码或制品。

| 微部署转换 | 条件与收敛 |
|---|---|
| disabled → preparing | enabled=true，微对象/版本/摘要及当前权限有效，父MPR安装精确匹配且可用 |
| preparing → ready | 当前MPR实例代次上的Prepare成功；准备句柄、微部署revision及版本绑定一致 |
| 任意可运行态 → waiting_runtime | 固定MPR停用、未就绪、实例退出或代次改变；撤销微路由并作废旧句柄 |
| waiting_runtime → preparing | 同一固定运行时部署/安装恢复，当前权限仍有效；重新Prepare原微版本 |
| preparing → failed | 摘要/源码/Schema/引擎条件不匹配或准备失败；明确记录原因，不换微版本或运行时 |
| ready/preparing/waiting_runtime/failed → disabled | enabled=false，先撤路由、取消在途纯计算，再幂等Release已持有句柄 |
| 任意未归档phase → archived | 微部署归档或所属MicroPlugin归档；撤路由、取消及Release后保留历史，不再启用 |

Release对象已不存在可幂等完成；不得因回收失败重新开放已停用路由。运行时离线时先持久化失效及待清理记录，恢复后清理旧代次，不阻塞内核管理API就绪。

微部署创建时enabled=false、phase=disabled；limits和父运行时绑定不可原地变更，修改需新微部署。MicroPlugin归档对关联微部署执行同样的撤路由/取消/释放规则，并收敛为archived。微调用正在执行时遇到微停用、归档、授权失效或父实例停止，取消传播到受限引擎并释放活动预算；`side_effect=none`也不能把部分JSON输出当成功。

batch/工作流引用的微Skill保持原版本/摘要/微部署。父运行时不可用时进入原任务的waiting_dependency，不自动改用另一个MPR或“最新”微版本。完整领域边界见[微插件架构](micro-plugins.md)。
