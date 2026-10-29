# 微插件、核心对象与共享运行时

[架构目录](README.md) · [领域对象](domain.md) · [状态机](state-machines.md) · [Skill定义](../skills/definition.md)

> 规格版本：2.6.0；状态：待实现的规范。本文描述核心扩展和运行时契约，不表示已有JS执行或外部搜索服务。

## 普通插件与微插件

普通Plugin以独立OCI制品安装，Deployment/Instance负责其容器、资源、工作负载身份及生命周期。微插件是核心独立管理的轻量业务对象，没有自己的OCI镜像、容器或实例证书。

Micro Plugin Runtime（MPR）本身是普通OCI预设`org.xelab.micro-plugin-runtime@1.1.0`。一个MPR实例可为多个微部署建立受限JS上下文；各微部署固定该父运行时的部署和安装，不在调用时寻找任意可用解释器。

```mermaid
flowchart LR
  U[用户 / batch / 工作流] --> S[核心SkillRouter]
  S --> A[skill.invoke + micro.invoke + micro.runtime.use]
  A --> D[固定微版本 / 制品 / 微部署]
  D --> M[MPR OCI实例]
  M --> J1[受限JS上下文A]
  M --> J2[受限JS上下文B]
  M --> HP[核心受控HTTP代理]
  HP --> EP[固定HTTPS提供者]
```

## 核心对象

| 对象 | 保存内容 | 不变量 |
|---|---|---|
| MicroPlugin | UUID、owner、name、status | 独立资源类型micro_plugin；默认私有，按具体UUID授权 |
| MicroPluginVersion | UUID、MicroManifest、package_digest、source_digest、runtime_installation_id | 发布后不可变；源码、Schema及运行时安装变化发布新版本 |
| MicroDeployment | UUID、owner、micro_version_id、runtime_deployment_id、enabled、limits、phase、candidate_skill | 固定微版本/limits/父绑定；父运行时实际安装须与版本runtime_installation_id一致 |
| HttpBinding | 微部署、endpoint_alias、secret引用、priority、enabled、timeout_ms及revision | 核心持有密钥引用；绑定只能使用微版本已声明的endpoint，VM不见secret ID或值 |
| 准备句柄 | MPR实例/代次、微版本/部署、制品摘要及运行状态 | 可丢弃重建，不是授权、制品或持久版本ID |

普通Installation只表示MPR镜像安装；它不表示后来发布的微源码受该镜像签名覆盖。微制品由核心按当前授权所有者及包/源码摘要单独登记，历史发布来源随版本保留。

## 发布、部署与Skill

1. 创建MicroPlugin时确定可信owner，不接受调用方冒充其他账户。
2. 发布版本时验证MicroManifest、源码、包摘要、输入/输出Schema和精确runtime_installation_id，保存不可变版本；此时不产生尚未绑定微部署的Skill。
3. 创建MicroDeployment，核对父部署micro.runtime.use和实际安装，固定微版本、runtime_deployment_id及limits后生成deployment.candidate_skill；初始enabled=false、phase=disabled。
4. registerSkill验证候选及来源权限；CreateSkill.installation_id必须为同一MPR安装。SkillTarget.kind=micro固定micro_version_id、package_digest和micro_deployment_id，plugin_id固定实际绑定运行时安装的plugin_id（官方预设为org.xelab.micro-plugin-runtime）。
5. 显式启用微部署和注册后的Skill；只有当前审批、父运行时健康和Prepare都满足时才可调用。

Skill的input_schema/output_schema直接来自不可变MicroManifest，mapping=identity。用户请求只包含业务JSON；微版本、制品、部署和控制句柄均由核心读取定义后构造，不嵌入可由业务字段覆盖的任意执行目标。

候选名为org.xelab.micro-plugin-runtime.micro.<micro_deployment_uuid>.run，版本为MicroManifest.version，超时固定为该微部署limits允许值。更换limits或父运行时绑定须新建微部署和候选；已有微部署只改变启停及运行观测，不原地修改这些绑定。

微对象的发布记录和候选来源由核心验证；候选不能随意更改目标或Schema后仍借原包摘要注册。MPR升级到另一安装时，需显式发布/迁移微版本和Skill，不把旧版本自动绑定到新版解释器。

## Prepare、Invoke与Release

MicroRuntimeService只接收内核mTLS调用。Prepare验证固定微版本、包/源码摘要、微部署、Schema和受限引擎条件，建立当前实例代次句柄；重复准备同一精确目标可幂等复用。运行时重启后核心按固定版本重新准备。

Invoke携带内核生成的可信微目标及业务input，要求与已准备句柄一致；核心和MPR均校验输入/输出Schema、期限和取消状态。成功只提交完整合法JSON结果。错误码和消息结构以机器RPC契约及预设接口为准，不能从stderr文本推断成功或副作用。

Release停止接纳该句柄的新执行并清理准备状态，重复清理或对象已不存在可幂等完成。Cancel/QueryResult复用CapabilityService，MPR的Describe/Initialize/Health/Drain/Shutdown复用普通PluginLifecycle。

用户无直接访问上述微运行时控制入口的路由。裸能力调用、HostService.Invoke或工作流裸contract不能绕过micro Skill和具体微对象权限。

## 执行profile与JSON协议

引擎为受限JavaScript，协议为json-stdio-v1，单次接收一份JSON输入并产生一份JSON输出，无流式业务输出。MicroManifest.execution_profile缺省解释为pure-json-v1；已有纯计算制品不通过回写默认字段重新打包，其source/package摘要保持不变。

| profile | 版本约束 | 执行能力与期限 |
|---|---|---|
| pure-json-v1 | side_effect=none，http_endpoints缺省或空 | 纯JSON计算，默认2s；无HTTP桥 |
| controlled-http-v1 | side_effect=non_repeatable，声明1..8个不可变http_endpoints | 监督器桥接至核心固定HTTP代理，总deadline最多15s、最多3次HTTP请求、单次最多4s |

每个endpoint固定alias、origin、path、method、auth_header/auth_scheme和payload_schema。origin只允许HTTPS443，方法为GET或POST；业务input不能覆盖地址、路径、方法、鉴权模板或新增endpoint。每个微部署的endpoint_alias绑定唯一。新增宿主能力必须发布新微版本/包摘要并显式部署审批，普通MPR升级不自动扩张已有微代码能力。

stdio描述受控输入/输出缓冲，不授予宿主文件描述符或进程能力。两个profile均无文件、Blob、真实fetch/socket、secret原值、直接HostService或任意Python/Node/shell入口。HTTP profile只增加host.httpBindings()/host.httpRequest(...)受控桥及声明式host.fail(...)；监督器持有RPC上下文，VM仍不能读取实例证书或委托句柄。

每个业务Call由核心冻结enabled HttpBinding的有序快照，按priority和UUID稳定排序。VM只得到alias、priority、timeout_ms投影；secret引用/版本及固定endpoint控制字段留在核心。快照不冻结撤权，禁用/删除绑定、secret撤销及微权限失效逐次检查，旧Call不得因配置变化静默换key。

监督器以HostService.ProxyHttp发送call_id、alias、sequence和有摘要的payload。核心从实际RpcDelegation推导MPR/微调用，联合检查调用者micro.http.request、原微执行权限、已批准绑定限定的内部secret.use和父MPR的micro.http.request/egress审批后才注入凭证。调用者不因此取得LeaseSecret权，原值也不交给MPR或VM；响应不含headers/Cookie，2xx JSON回显原凭证时拒绝返回。

核心持久台账以(call_id,sequence)去重，同指纹重放原结果、异内容冲突；sequence为1..3，每alias最多一次真实请求，同一Call只允许一个在途HTTP。实际已发送但结果不能确定时sticky unknown，禁止后续sequence和自动重跑脚本；已确认的预发送失败或上游429/5xx/鉴权拒绝可由脚本在同一Call明确fallback，平台授权失败不能当作备用条件。

MicroManifest.business_errors可选声明业务code、http_status及retryable；受控VM的host.fail只能选该清单，产生MicroInvokeResponse.error。业务状态只允许400/409/413/429/502/503/504，不能声明平台保留码或401/403。代理台账unknown优先于业务失败或stdout，核心固定为UPSTREAM_OUTCOME_UNKNOWN/409，不伪装成功JSON。

MPR执行引擎实施受限堆、执行预算、超时中断和上下文隔离。父容器cgroup约束共享进程的CPU/内存硬上限；不能仅因存在父容器就声称每段JS拥有独立容器或独立内核级进程隔离。引擎及MPR属于执行隔离实现的可信基础。

## 两级预算与权限

执行顺序为先父MPR实例活动槽和内存预留，再子MicroDeployment活动槽和执行限制。所有活动微执行、代码准备缓存及MPR自身开销共同计入父部署limits；申请失败释放先前预留，不等待时持有无限子队列。

微部署不建立额外OCI实例，不重复申请节点插件池预留，也不借用内核至少4C16G。多个微对象不能各自复制父部署预算；增大微限额不自动扩容MPR。

创建微部署及每次执行都检查父runtime_deployment_id上的micro.runtime.use；执行还要检查skill.invoke、micro.invoke（具体micro_plugin UUID）、父MPR当前审批、安装绑定和完整授权链。HTTP调用另需micro.http.request及当前HttpBinding/secret/egress准入；绑定管理需micro.http.bind，创建或更换secret还需secret.bind。子账户/Key和同实例关系不产生隐式共享，脚本也不能继承MPR自身HostService能力。

## 故障、停用和恢复

MPR未安装、停用、离线或代次变化时，关联微部署进入waiting_runtime，旧准备句柄及新调用路由失效；核心管理API和CLI继续Ready。恢复后仅为同一固定父部署/安装重新Prepare，不能自动改用其他MPR或同名微版本。

微部署停用时收敛到disabled，微部署或MicroPlugin归档时收敛到archived；两者先撤新路由、取消在途执行，再Release句柄。已发HTTP不能因取消被宣称撤回，证据不足保留unknown；不能以清理超时为由重新开放路由。微版本、历史Call及审计记录按既有保留规则保存。

batch/工作流继续固定原微Skill、版本、制品和微部署，父运行时不可用时等待原依赖。纯profile安全重算仍保留原定义、输入、业务键和deadline；HTTP profile按non_repeatable处理，不自动换attempt重跑未知外部请求。归档及权限失效按取消/授权暂停规则收敛。HTTP执行细则见[MPR协议](../preset-plugins/micro-plugin-runtime/execution.md)。
