# 微插件权限与隔离边界

[MPR目录](README.md) · [身份权限](../../identity/README.md) · [容器隔离](../../runtime/isolation.md)

> 规格版本：2.8.0；状态：待实现。运行任意宿主解释器子进程本身不是本规范所要求的隔离。

## 核心对象与来源

微插件、版本和微部署由核心管理，owner来自可信认证上下文，不能由脚本输入自报。角色、权限组、子账户和API Key仍按完整来源链求交；调用某微版本不隐含发布代码、改变微部署或执行其他微插件的权限。

MicroDeployment创建时才生成候选Skill，初始disabled，不自动注册或启用；注册后固定`target.kind=micro`、版本、package_digest和micro_deployment_id。核心联合验证`skill.invoke`、真实微对象上的`micro.invoke`及父部署上的`micro.runtime.use`，另检查当前微部署及Runtime绑定；脚本输入中同名ID、owner或权限字段仅是业务数据，不能覆盖可信绑定。

用户代码的信任来源是核心记录的制品上传、owner审核与内容摘要。MPR的OCI签名只覆盖运行时自身，不能为后来上传的脚本背书。注册、审核或启用其中一个对象也不会自动扩张另一个对象的权限。

## 脚本可见能力

默认pure-json-v1只提供受限JavaScript及虚拟JSON I/O，side_effect=none、http_endpoints为空；2s纯计算期限不因MPR升级而扩张。两个profile均不暴露普通Node/Python模块、filesystem、真实network/fetch/socket、process、宿主命令、容器接口、直接HostService或secret原值。脚本不能继承Runtime的mTLS私钥、源凭证、授权句柄或环境变量。

没有通用require/import宿主模块、文件路径读取或启动进程的桥。脚本生成的路径、URL或“下一步命令”仍只是JSON返回值，平台不能把它当成额外执行授权。

MPR1.1.0另支持显式controlled-http-v1。它必须声明side_effect=non_repeatable与1..8个不可变endpoint，每项固定alias、origin、path、GET/POST方法、鉴权头模板和payload_schema；不得由业务JSON覆盖或临时拼接URL。新增endpoint/profile须新包摘要和显式部署审批，已有pure制品省略字段仍解释为pure，原摘要不变。

MPR1.2.0增加微部署network_enabled手动开关，默认false，只有controlled-http-v1可设true。它只允许已声明HTTP桥，不授予真实网络或放宽endpoint/secret/egress规则。Prepare与Ready可以离线完成；关闭状态的HTTP业务Invoke在启动worker前以PERMISSION_DENIED拒绝，violations.field=network_enabled，不占HTTP序号，不伪装成NO_PROVIDER后fallback。

## 受控HTTP不是网络授权

HTTP profile只接入host.httpBindings()、返回Promise的host.httpRequest(alias,payload,sequence)及受声明约束的host.fail(code,message)。监督器代调用HostService.ProxyHttp；VM没有RPC客户端、Call委托、secret ID/版本/值，不能传任意请求头。pure profile不能借MPR可选的micro.http.request代理审批调用这些桥。

核心仅向VM投影本Call固定绑定快照中的alias、priority和timeout_ms。绑定priority变化只影响新Call，禁用/删除立即阻止新转发；secret或绑定实质内容变化不能让旧Call静默改用新凭证，需明确STATE_CONFLICT或当前授权错误。

每次ProxyHttp从实际证书与RpcDelegation核对MPR实例、父调用、微对象/部署/版本，联合检查当前network_enabled、微部署启用状态及原generation、调用者micro.http.request、micro.invoke、micro.runtime.use、固定endpoint、已批准绑定限定的内部secret.use及当前状态、父MPR的micro.http.request/egress审批和完整来源链。终端/MPR不因此取得secret原值或LeaseSecret权。正文call_id/alias仅为待验证目标，不是身份凭据。

核心完成上述检查才注入鉴权凭证。请求仅允许固定Accept/Content-Type和版本声明的鉴权模板；VM不能提供Host、Cookie、代理认证或其他任意头。核心代理不把LeaseSecret返回给MPR，也不在审计、错误、HTTP台账或VM回调中保存原值。

外部请求仅允许HTTPS443、固定来源与路径，关闭重定向，验证TLS证书及主机名。DNS解析结果和实际连接IP均检查，拒绝私网、loopback、link-local、宿主及其他插件内网；每次新连接重新检查，DNS变更不能绕过egress限制。

GET仅将符合payload_schema的扁平string/int/bool对象按固定查询编码器生成参数；POST发送受验证I-JSON。请求最多64KiB、响应最多512KiB，单次最多4000ms且受剩余总deadline约束；不返回上游headers、Set-Cookie或非2xx原文。2xx JSON任意字符串包含本次注入凭证原值时拒绝为UPSTREAM_RESPONSE_INVALID。

同一Call最多3个sequence、每alias最多一次真实请求、最多1个在途请求。台账同指纹重放只返回原记录，不重新出网；异内容拒绝。未知是否已被上游处理的已发请求使Call进入unknown并阻止后续sequence，不能以换alias、换seq或重启脚本掩盖。

host.fail仅接受MicroManifest.business_errors中声明的code；业务状态限400/409/413/429/502/503/504，禁止平台保留码和401/403。它只选择有界业务错误，不增加HostService权限；脚本不能伪造平台鉴权失败或通过正常stdout覆盖已报告失败。核心台账unknown始终优先，固定为UPSTREAM_OUTCOME_UNKNOWN/409。

## VM与执行单元

每次调用必须在独立短命worker进程内建立新VM，不保留跨请求可变状态。引擎限制heap、stack、编译/求值资源并支持中断；监督器独立执行硬wall期限，核心可按已验证进程身份终止worker，不能让native操作卡住整个MPR控制线程。

worker不得读取监督器内存、其他用户VM、运行时凭证或制品存储。执行单元边界必须实际限制这些访问；仅在同一权限环境启动普通Python或Node子进程，不构成这一边界，也不能宣传为本版支持的engine。

核心受审OCI适配器管理父MPR下control/workloads及每微部署子cgroup；MPR没有privileged、可写宿主cgroup或任意cgroup路径/PID的控制权。父域和workloads内部域不驻留进程，监督器/cache在control，worker在明确微部署叶子域，满足cgroup v2资源控制拓扑。

worker只在核心验证实际PID属于该MPR实例/代次、正确挂入微部署子组并应用CPU/内存限制后才运行不可信JS。使用pidfd或等效稳定身份防PID复用；不能仅信任MPR报来的宿主PID、路径或已附加声明。缺少cpu/memory控制器、可靠入组、组终止或回收能力时拒绝Ready，不退回共享进程内VM冒充资源隔离。

实际通道为监督器创建停等worker，经HostService.AttachMicroWorker提交当前Call委托、核心group不透明句柄、容器PID与启动时间；核心按namespace/start-time/pidfd核实，再迁入组并Ack。仅有当前微调用可使用对应句柄，worker在Ack前无用户代码；失败终止/回收，不接受业务JSON选择cgroup路径或宿主PID。启动阶段按control预算受限计量。

resources默认250mCPU/256MiB，是该微部署全部活动worker共享的CPU带宽/计量内存上限。parent/control/workloads及微部署限制共同生效；内存计量包含RSS等，不等同JS heap，worker虚拟地址空间1GiB也不是RSS或预留。不同微部署有独立子组，同一微部署多个Call故意共享一份资源故障域。

微部署cgroup OOM按组终止相关worker，核心依据memory.events等证据终结对应Call；已发HTTP仍按unknown处理。单Call取消只终止对应worker，同组OOM可影响该组其他Call；其他微部署不因这个子组OOM被直接组杀，父实例整体故障仍可能影响所有微部署。

首个worker激活时计一次微部署资源；最后worker退出并确认空组/回收后释放。残余进程或计量内存未处理前继续计入父workloads，不能提前释放预算。脚本不能自行移动进程、创建资源组或逃出父限制。

停用以enabled=false/generation++原子撤路由并进入draining，取消队列/Prepare/worker及句柄，组清理完成才为disabled。失败保持draining/failed，不允许修改settings；只有已排空disabled可手改资源/网络。组heap/stack/输入输出等可计量预算乘允许并发须不超过组memory，native开销仍以hardmax和实测核验。

## 每次转发重新验证

Prepare/Invoke/Release只接受核心可信mTLS调用。其他插件不能凭自己的实例证书直接调用并自报微版本或subject；MPR也不能把“上次准备成功”当永久执行授权。

核心在每次执行前检查账户/祖先/源凭证、微版本与微部署状态、Skill固定摘要以及Runtime当前安装/代次。MPR核对受信封装、制品摘要和当前执行身份；脚本不接触这些控制字段。

归档微插件/微部署、停用微部署、撤销HTTP权限/绑定/secret或Runtime使新调用被拒绝/等待；在途按取消和排空规则收敛。已发请求的外部效果不能靠本地取消撤回，证据不足保留unknown。旧执行单元、旧缓存或迟到结果不能在撤权后继续创建调用或覆盖新状态。

制品缓存只保存不可变代码和受审元数据；按摘要复用不共享用户输入、输出、VM全局变量或授权上下文。错误与日志按原调用权限隔离，不能通过缓存命中信息枚举其他用户的私有代码。

MPR1.3另可申请skill.invoke代理批准，仅供ExtensionHost监督器执行固定闭包handler；VM没有InvokeSkill客户端。核心拒绝普通micro Call或不匹配闭包handler的请求，未批准可选代理时只禁用skill扩展模式；data扩展和原纯计算继续按各自权限运行。
