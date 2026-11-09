# M4 HostService 与固定调用代理

[实施目录](README.md) · [目标 #51](https://github.com/jyxxe/XeLab/issues/51) ·
[阶段 #53](https://github.com/jyxxe/XeLab/issues/53)

## 实现范围

普通 OCI 插件的 HostService 提供 Resolve、Invoke、InvokeStream、
ListSkillCandidates、ResolveSkill、CheckSkillAccess、InvokeSkill 和
InvokeSkillStream。事件 RPC 的独立实现和证据由事件阶段记录；MPR、
Extension Plugin、TaskEngine 不在这一普通插件执行 profile 内。

Resolve 只返回不透明 binding_id、精确契约及提供者部署标识，不返回内网地址。
请求/部署绑定复用依赖解析器的固定安装、配置、代次及期限；每次使用重查，
不因提供者重启或安装升级静默切换。Skill 固定 UUID、定义摘要、安装和签名目标，
能力或 HTTP 目标继续使用原 Broker 的并发、资源、额度、Schema、unknown 和结果核实。

## 授权与来源

- 实际工作负载来自 mTLS SAN，instance/generation 必须匹配正在运行的 Call、
  活跃 RpcDelegation 和 ExecutionDelegation。正文不能覆盖主体或工作负载身份。
- Host 代理范围为签名 requested_rules、独立当前审批、调用者当前来源和原调用
  冻结上限的交集。可选请求未批准不能调用；必需代理请求未完整批准不能启用。
  原有可选 secret.use profile 保留，不自动获得新动作。
- 代理审批保留真实审批凭证、授权来源和浏览器 Origin。原来源撤销后新增同权
  授权不能复活原审批或原调用。权限仍由当前事务实时检查。
- 每个业务子调用新建 Call 和实例句柄；host_child_calls 记录祖先关系。
  所有祖先的撤权、收窄、取消、代次和期限继续限制后代。实例路径重入或超过
  16 层拒绝；插件不能通过递归复制实例并发或调用额度。
- 原调用者来源与每层代理审批分别持久冻结，调用时在实际资源及其所有者上取
  交集。`owned_by` 与固定 ID 上限保持独立约束，不能因提前合并选择器丢失
  所有者，也不能把两个约束改为任一满足。
- 内部 host_call_tickets 仅保存 HMAC 校验摘要及明确目标、输入摘要、固定依赖
  绑定、父调用和冻结上限，不保存用户令牌或原始 RPC 句柄。私有 Broker 准入
  标识只在内核内存出现；发送给目标插件的是新的不透明委托。

## 发现、幂等和流

Skill 候选只包含获准读取的已注册 capability 型候选，按插件、名称、版本、UUID
稳定排序。签名游标绑定主体、凭证、过滤条件和安装/注册水位，15 分钟到期；
水位变化必须重新首页。ready 只是当前可用性，不能替代调用时授权。
候选列表与依赖 Resolve 在数据库等待后、提交返回前重新核实实际父调用、原始授权
来源与全部代理上限；候选逐项复查 skill.read，过期的 Resolve 不提交临时绑定。
该复验共用原有事务内证明缓存，仍受原有 2 秒元数据 RPC 预算约束。
CheckSkillAccess 不创建 Call 或资源预约，仍同时检查 Skill 和原目标动作。

子调用幂等键绑定父 Call；重放保持原目标、输入、单次/流响应模式和第一次准入参数，不能重派
已准入的 unknown 工作。同一准入记录最多关联一个子 Call。每个父调用最多保留 256 个子调用准入记录；重放不
增加条目。Invoke 使用请求给定的新 Call UUID；Skill 也可显式提供新的 Call UUID，省略时由核心创建。
错误的 `xelab-error-bin` 在原调用者仍可核实时保留已准入 Call 引用；传输丢失时
错误元数据也可能丢失。调用方提前选择 UUID 可配合既有 getCall/运维核实，不表示
可以自动重派 unknown，也不新增插件查询 RPC。

RPC 流复用原 Broker 的 Schema 校验、断开取消和物理槽围栏。64 个并发响应，
解码前跨所有连接共享 64 个准入槽，每层最多两个缓冲帧；item 业务 JSON 最大 1 MiB，completed 最大 16 MiB。
每次实际交付缓冲帧前重查原父链和子调用来源，撤权后排队数据不能继续交付。
等待、授权复查和交付共享最初的父调用、Skill/Invoke、传输及上下文截止时间；
流完成帧排队后也不能越过该期限。每次复查最多使用原剩余预算中的 10 秒，不重设流期限。
同一授权事务内复用的证明仍检查来源凭据及账户的当前到期时间，业务来源与代理来源分别核实。
SSE 原有 1 MiB 帧限制保持不变。客户端断开不能自动重派有副作用业务。

## 迁移与验证边界

Schema18 追加私有准入、调用祖先表和独立代理上限快照，不改写历史 SQL、校验和或已签名制品。
恢复时历史委托失效，旧准入标识不能在新宿主复活。

组件测试使用真实 PostgreSQL、实际 mTLS HostService/CapabilityService 和
受控回环提供者，覆盖发现/检查/单次/流、重复请求、错误身份/摘要/绑定、
旧代次、父终止、祖先收窄与过期、冻结上限和撤权。元数据来源是明确测试
fixture，不能把它称为签名 OCI 或独立宿主生产验收。

实际执行命令、精确 SHA、CI 和剩余项由关联 PR 及 M4 验收记录补充。
AWS 保持 DEFERRED；生产 RUN-03、daemon/off-host 正式验收由
[#50](https://github.com/jyxxe/XeLab/issues/50) 独立跟踪。
