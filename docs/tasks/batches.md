# 持久化批次调度、授权与恢复

[任务目录](README.md) · [HTTP协议](../skills/batches.md) · [BatchEngine内部方法](../internal/BatchEngine.md)

> 规格版本：2.8.0；状态：待实现的规范。

## 存储与执行器

skill_batches和skill_batch_items见[核心SQL](../contracts/metadata.sql)。批次不要求用户先创建Workflow，不占用普通DAG的64步定义上限；它是使用同一租约、fence、Broker与授权逻辑的另一种调度适配器。管理Operation只处理安装/备份等控制动作，不承载批量业务结果。

BatchEngine.submit固定Skill版本、安装制品、输入摘要和ExecutionDelegation，提交项集合不可增删或改写。claim以SKIP LOCKED领取queued项，事务内锁批次计数并计算max_parallel及账户树预算；持久队列不等于一次向插件灌入1000个RPC。

每次领取产生execution_id、单调fence、30s租约；核心执行器每10s续约。有界Skill仍由Broker调用，不要求embedding/知识库实现TaskWorker.Execute。续约/完成同时核验execution_id/fence/租约；外部调用不持有数据库长事务。

## 状态与汇总顺序

| 批次state | 进入条件与后续 |
|---|---|
| queued | 已提交，尚无活动项；可领取 |
| running | 有活动项或仍可领取待处理项 |
| waiting_dependency | 固定Skill/提供者暂不可用；等待同一制品，不换模型 |
| paused_authorization | 执行授权失效，停止新项并取消在途子调用，保留结果 |
| cancel_requested | 用户请求取消；停止领取，等待在途退出/结果核实 |
| unknown | 存在无法证实副作用的项目；停止继续领取，不自动重放 |
| succeeded | 全项succeeded |
| partially_succeeded | 无未知/活动/待处理项，部分succeeded且部分failed/canceled，未显式整批取消 |
| failed | 无成功项且存在失败；超时未开始项记录DEADLINE_EXCEEDED后按实际结果汇总 |
| canceled | 显式取消已收敛且无unknown；已成功项仍保留 |

汇总优先处理unknown，再处理cancel_requested/paused_authorization等阻塞状态。只有明确无在途/未知结果才进入普通终态；一旦普通终态，禁止resume或原地改输入。unknown不是已完成终态，不设置7天结果到期时间，需核实后再汇总。

counts中queued/running/succeeded/failed/canceled/unknown相加等于total，同一短事务提交状态和计数。失败导致stop时尚未执行项记canceled并带原因，不混作成功；done为succeeded+failed+canceled，UI把unknown单列。

截止时间到达停止新项、取消在途；已经提交的实际结果按证据保存，超时未开始项记failed。仍无法证实副作用时保持unknown，不用failed覆盖未知。最终无未知时按实际各项结果汇总；批次error记录整体DEADLINE_EXCEEDED，即使存在部分成功。

## 执行授权

提交同时检查batch.create、skill.invoke、目标动作、Blob白名单、插件审批、完整祖先/源凭证和客户端上限。默认run_as=user；关闭页面不影响已持久化队列，但不会让令牌/会话变成永久授权，源凭证到期或撤销后暂停。

需要跨登录长期执行时，显式采用已批准服务账户的执行委托；service_account_id不是任意替身。owner始终为提交者，subject为获准实际用户或服务主体，知识库owner/indexer ACL按subject检查，不因为batch由管理员提交而旁路。

每项领取、租约续约、插件子调用与结果提交重新检查当前授权。权限失效可能发生在数据库写入之后，记录unknown并通过受控QueryResult核实；停止授权不能伪称已回滚副作用。

resume只接受原owner和当前有效来源。显式新建执行委托，以原固定上限∩新来源有效授权替换失效凭证关系，保留原subject/获批服务账户和整体deadline；来源更新有单独审计。只恢复未开始项，失败/取消/unknown项不重放。不同owner或切换run_as需新批次和新授权。

读取批次按batch.read范围；父子账户不自动共享结果。领取结果另检查当前skill.invoke、目标与对应数据资源读取权；状态读取不返回原输入或明文结果。API Key不自动得到batch相关动作，需明确选入其继承上限。

## 重启、去重与清理

业务键为sha256(JCS([batch_id,item.id,固定Skill摘要]))，在全部物理尝试中稳定。执行器崩溃后等待旧租约过期，提高fence再核实Call；none可安全重算、idempotent按原键去重、queryable先QueryResult，non_repeatable无法核实则unknown。默认一次尝试；可安全恢复的总物理尝试最多5次，唯一决策者为BatchEngine，Broker对batch不得叠加自己的重试。

QueryResult可证实的unknown由核心自动收敛并标记resolution_source=automatic。否则使用[resolveSkillBatchItem](../api/operations/batches/resolveSkillBatchItem.md)人工核实：独立batch.resolve、当前原业务权限、ETag和审计证据缺一不可；确认成功必须给符合原输出Schema的output，核心使用原Call身份封装，UI标记manual。确认未执行则记failed/CONFIRMED_NOT_EXECUTED，后续重做需新批次。该接口不恢复未知项目执行，也不向iframe桥开放。

完成先写核心私有不可变结果对象，再在有效fence下提交引用/摘要/字节数、项结果、批次计数与outbox。对象键不向浏览器暴露，不生成公开下载URL。重复完成幂等，旧fence迟到结果不覆盖，孤儿对象由GC清理。

结果对象和持久化输入使用独立批次存储计数，初始硬上限为每账户树1GiB，并服从节点磁盘水位。提交与结果写入前按根到叶锁定/预留并累计所有祖先，防止创建子账户复制磁盘预算；超额项目以QUOTA_CONFLICT/RESOURCE_EXHAUSTED失败，业务可能已提交时先核实且不重放副作用。

每账户树最多100个未完成批次、10000个未确认完成项（含unknown），节点最多1000批次/100000项，达到上限返回429 RATE_LIMITED及Retry-After。上述存储/队列是初始硬上限，改变需配置契约新版本。输入GC置null，保留摘要/字节计量历史；已清理对象不再计入当前使用量。

## 微插件执行项

micro Skill同样可提交batch；每item固定微版本/包/微部署，缺失运行时等待相同固定依赖，不换安装。单项默认时限省略时取min(120000,Skill.maximum_timeout_ms)，微脚本默认2s；显式超Skill上限拒绝。核心执行器不能把批次期限7天解释为单脚本可运行7天，取消传播到VM/worker。
