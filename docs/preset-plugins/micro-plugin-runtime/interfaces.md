# MPR接口与固定Skill转发

[MPR目录](README.md) · [HTTP接口目录](../../api/README.md) · [核心字段模型](../../api/models/README.md) · [RPC目录](../../rpc/README.md)

> 规格版本：2.8.0；状态：待实现。核心HTTP负责微对象、运行设置和HTTP绑定管理，Runtime只实现受控执行与监督器桥。

## 核心管理职责

| 操作 | 必须确定的对象与行为 |
|---|---|
| 创建微插件 | 核心分配ID并固定owner；创建不自动获得可执行版本 |
| 发布微版本 | 校验输入/输出Schema、engine、协议、代码制品及package_digest；版本不可原地替换 |
| 创建微部署 | 绑定明确的Runtime安装/部署与微版本，保存固定并发/执行上限，生成candidate_skill；初始disabled，不自动注册或启用Skill |
| 启停与归档 | 执行权限、当前revision/CAS和来源链检查；撤销新执行入口并保留历史 |
| 查询状态/结果 | 按真实owner/资源范围过滤，核心Call为权威执行记录 |

精确路径、请求字段、响应、权限动作和ETag/幂等条件以[核心逐接口](../../api/README.md)与[模型](../../api/models/README.md)为准。MPR不发布用户可直接选择source、路径或解释器的管理/执行Skill。

上传代码所用Blob仅是导入来源，核心核验并持久保存不可变制品后形成版本。后续执行不依赖原上传Blob的剩余TTL，不把用户任意Blob引用当成已批准处理器。

## 每微版本Skill

发布MicroPluginVersion只保存不可变制品，不产生未绑定微部署的候选。创建MicroDeployment后，核心依据其UUID、固定微版本和limits生成candidate_skill；调用者另行按原样registerSkill，并分别启用微部署和Skill，不能把生成候选当自动开通执行权。

`target.kind=micro`固定微版本、package_digest和micro_deployment_id，另由微部署固定Runtime安装/部署。调用方只提交业务input，不能根据input中某个ID换脚本或解析latest。原limits和父绑定不可变，修改须新建微部署；resources与network_enabled可在停用排空后经settings独立调整，不改candidate_skill定义。

执行必须同时拥有`skill.invoke`、真实micro_plugin上的`micro.invoke`、父Runtime deployment上的`micro.runtime.use`，并满足当前来源链与Runtime审批。创建/启用微部署不能暗中开启父Runtime；普通执行权不授予发布、部署管理、源码或诊断读取权。

业务input/output按微manifest Schema定义，资源标记和执行上限受该版本及当前授权约束。普通Skill调用、工作流和异步batch仍使用核心入口；Runtime不向用户暴露可覆盖绑定的通用execute能力。

MicroManifest.execution_profile省略解释为pure-json-v1，http_endpoints为空、side_effect=none；MPR1.1.0新增的controlled-http-v1须显式声明1..8个endpoint与non_repeatable。origin/path/method/鉴权模板/payload_schema固定在微版本中；新增宿主能力需新制品摘要和显式审批，不能只改调用input。

核心负责把业务input与可信执行绑定组成受控RPC封装；这个封装不能由脚本或普通插件客户端自行构造。它是micro目标的显式转发语义，不表示内核执行JavaScript。

## 微部署settings

`PUT /api/platform/v1/micro-deployments/{id}/settings`，请求为`{resources:{cpu_millis,memory_bytes},network_enabled}`；携微部署If-Match和Idempotency-Key，按核心模型执行管理权限及父Runtime使用权/资源审批检查。

resources默认250mCPU/268435456字节（256MiB），指该部署所有活动worker的合计上限；network_enabled默认false，true仅适用于controlled-http-v1。只有enabled=false、phase=disabled且无worker、Prepare、queued请求时可改，否则STATE_CONFLICT；同值请求无副作用。

停用先原子enabled=false/generation++并phase=draining，撤路由、取消/回收队列、Prepare、worker、句柄及组；全部完成才disabled，失败保持draining/failed并拒绝settings。组内heap/stack/输入输出缓冲预算乘允许并发还须不超过resources.memory_bytes。

成功更新revision/generation，旧准备句柄失效；重新启用后按新设置Prepare。原微版本/limits/父绑定不变，候选Skill不漂移。关闭联网的HTTP微部署可以Prepare/Ready，但业务Invoke在worker前PERMISSION_DENIED，violations.field=network_enabled，不消耗sequence或触发fallback；pure调用照常。

## Runtime RPC

| RPC | 受控行为 |
|---|---|
| [MicroRuntimeService.Prepare](../../rpc/MicroRuntimeService.Prepare.md) | 接收并核验核心已批准的固定制品/Schema/引擎约束，准备有界缓存；不提前运行业务脚本 |
| [MicroRuntimeService.Invoke](../../rpc/MicroRuntimeService.Invoke.md) | 核对可信绑定后在新VM执行一次JSON请求，校验完整结果；不接受即时source/path/argv/env |
| [MicroRuntimeService.Release](../../rpc/MicroRuntimeService.Release.md) | 幂等释放本实例对应准备状态/缓存引用；不删除核心微版本或历史结果 |

三者只接受当前核心可信mTLS身份，不把其他插件的工作负载证书当核心身份。字段与期限以Proto和逐RPC语义为准；Prepare成功不构成后续Invoke的可复用授权票据。

取消和核实复用[CapabilityService.Cancel](../../rpc/CapabilityService.Cancel.md)、[CapabilityService.QueryResult](../../rpc/CapabilityService.QueryResult.md)，调用ID映射到准确的微执行单元。Lifecycle仍实现现有Describe/Initialize/GetHealth/Drain/Shutdown，不为每个脚本创建一套容器生命周期。

## 核心资源组与AttachMicroWorker

Runtime内部方法ensure_micro_pool、configure_micro_group、attach_micro_worker、release_micro_group负责父树、微部署组、受验证worker入组与回收；这些是核心受审适配器方法，不给VM或MPR任意cgroup写入口。

HostService.AttachMicroWorker由监督器在当前微Call委托下提交核心group不透明句柄、容器PID和进程启动时间。worker必须停等且未加载用户代码；核心核对namespace/start-time/pidfd、Call/微部署/父实例代次并迁入正确组，Ack后才可执行JS，失败终止回收。启动过程计control预算，句柄不是跨Call授权。

资源组按活跃微部署计一次，默认父4C4GiB中workloads为3584MiB/3500m，control为缓存256MiB+控制128MiB/500m。默认每微部署250m/256MiB、并发1，实例最多4活动槽；同微部署多个worker共享子组限额，最后worker退出且清理确认后释放资源。

## 核心HttpBinding接口

以下路径相对于`/api/platform/v1`；id为实际MicroDeployment UUID，binding_id须属于该部署。读取需micro.http.read，写入需micro.http.bind，并重新检查完整来源链及父egress审批；创建或更换secret引用另需该secret的secret.bind。

| 方法/路径 | 请求与并发条件 | 响应/行为 |
|---|---|---|
| GET /micro-deployments/{id}/http-bindings | 按当前可见范围读取 | 绑定列表及版本元数据，不回显secret原值 |
| POST /micro-deployments/{id}/http-bindings | HttpBindingSpec、微部署If-Match、Idempotency-Key | 创建固定endpoint_alias的凭证使用绑定；默认disabled |
| GET /micro-deployments/{id}/http-bindings/{binding_id} | 读取真实所属绑定 | 绑定对象与ETag，secret引用仅向获准管理方展示 |
| PUT /micro-deployments/{id}/http-bindings/{binding_id} | 完整HttpBindingSpec、绑定If-Match、Idempotency-Key | CAS全替换；priority只影响新Call，密钥/内容变化不得替换旧Call快照 |
| DELETE /micro-deployments/{id}/http-bindings/{binding_id} | 绑定If-Match、Idempotency-Key | 撤销后立即阻止新转发，保留历史Call/代理台账 |

HttpBindingSpec字段为endpoint_alias、priority(0..1000)、enabled(默认false)、secret_id、timeout_ms(100..4000，默认4000)。每个微部署内endpoint_alias唯一，必须已由该微版本声明，不能借绑定提交新origin/path/header。精确状态码、模型和分页字段以[核心逐接口](../../api/README.md)为准。

每个业务Call由核心冻结enabled绑定/版本与secret引用/版本，按priority和UUID稳定排序；VM的host.httpBindings仅给alias/priority/timeout_ms。绑定禁用/删除或secret失效逐次生效，快照不是持续授权；配置实质变化旧Call返回STATE_CONFLICT，不静默更换凭证。

## HostService.ProxyHttp

监督器发送当前RpcContext、call_id、alias、sequence(1..3)、payload_json及payload_digest。核心从实际RpcDelegation核对MPR/微调用、固定endpoint和绑定快照，验证当前network_enabled、enabled/原generation、调用者micro.http.request、micro.invoke、micro.runtime.use、已批准绑定限定的内部secret.use及当前secret状态、父MPR的micro.http.request/egress审批后注入凭证。终端只需微HTTP及原微执行权限，不因此取得secret原值；MPR不请求secret.use或LeaseSecret。

HTTP代理只允许固定HTTPS443 GET/POST，无重定向/私网目标。GET把扁平string/int/bool payload编码成固定query，POST发送I-JSON；允许请求头仅为固定Accept/Content-Type及核心注入鉴权。请求≤64KiB、响应≤512KiB、单次≤4s，整体≤15s。

返回MicroProxyResult：operation_id(UUID)、outcome(completed/not_sent/unknown)、status_code(nullable100..599)、body_json(nullable JSON)、error_code(nullable string)、retry_after_ms(nullable0..60000)、duration_ms(0..4000)。无响应headers；仅2xx有body_json，非2xx原文和Cookie不返回，回显本次secret原值的2xx JSON拒绝为UPSTREAM_RESPONSE_INVALID。

同(call_id,sequence)同指纹重放原记录而不再出网，异内容冲突；每alias最多一次真实请求，单Call最多3次且最多1个在途。unknown阻止后续sequence且使核心Call保持unknown，不能由脚本结束改写。仅同Call内明确的预发送失败/上游429、5xx、认证拒绝可按有序备用alias继续；平台鉴权错误与未知是否已执行的结果不得自动重试。

## 声明业务错误

MicroManifest.business_errors可声明{code,http_status,retryable}，仅允许400/409/413/429/502/503/504，禁止平台保留码或401/403。controlled VM的host.fail(code,message)只选已声明code并产生MicroInvokeResponse.error，不是新增HostService权限。无声明码的任意JSON错误字段不能改变核心成功/失败状态。

核心HTTP代理台账unknown优先，固定为UPSTREAM_OUTCOME_UNKNOWN/409；即使脚本catch、host.fail或输出正常JSON也不变成成功，亦不默认换attempt重跑non_repeatable调用。

## 状态、并发与失败

Runtime安装、部署或实例不可用时，相关微部署进入waiting_runtime或对应明确失败状态；不能把固定目标自动切到另一安装或未审核Runtime。Runtime恢复仍需核对原绑定和当前代次。

输入/Schema错误、未批准制品、权限失效、队列满、执行异常、硬超时、内存/栈/输出越限均通过规范错误返回；不把空输出或worker被杀标记成功。结果已保存时按核心Call/幂等语义读取；pure按无外部副作用规则核实，controlled-http按non_repeatable和持久HTTP台账处理unknown，禁止自动重跑。

MicroRuntimeService.Invoke响应以oneof outcome携带result或error，已识别Call的脚本失败可保留stderr诊断；核心必须把error记为failed，不能因gRPC传输成功而返回业务成功。鉴权/封装/传输失败使用gRPC错误；超时或崩溃无诊断时按Call状态处理，诊断非成功前提。
