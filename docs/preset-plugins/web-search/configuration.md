# 网页搜索配置与HttpBinding

[网页搜索目录](README.md) · [服务商适配器](providers.md) · [执行协议](execution.md) · [密钥托管](../../runtime/secrets.md)

> 规格版本：2.8.0；状态：待实现的规范性配置设计。

## 微版本配置

本预设除HttpBinding外不定义额外普通配置；`defaults.json`为`{}`，由`defaults.schema.json`约束为空对象，不向MicroManifest添加未声明的config_schema字段。API Key、服务商列表、目标URL和认证头不能放进微版本配置、Skill业务输入、源码常量或普通部署环境变量。

资源和联网开关由核心MicroDeployment.settings管理，独立于上述空普通配置和不可变MicroManifest。

不可变MicroManifest声明`execution_profile=controlled-http-v1`及`brave`、`tavily`两项`http_endpoints`。修改方法、origin、path、认证位置或payload_schema必须发布新微版本，并按原流程创建新的微部署和候选Skill；HttpBinding不能修改这些制品内容。

## 微部署资源与联网设置

MPR 1.2.0下默认resources为`{cpu_millis:250,memory_bytes:268435456}`（256MiB），network_enabled=false。资源硬上限合计覆盖该微部署所有worker；默认活动并发1/硬上限4共享预算，JS heap默认16MiB仍是每VM子上限。核心适配器配置逐微cgroup，无每微OCI，无MPR宿主cgroup管理权。

| 操作 | 定义 |
|---|---|
| 方法 / 路径 | `PUT /api/platform/v1/micro-deployments/{id}/settings` |
| 正文 | `{resources:{cpu_millis,memory_bytes},network_enabled}`，完整提交该组设置 |
| 请求条件 | 当前If-Match、Idempotency-Key、部署管理权限及来源链 |
| 可写状态 | enabled=false、phase=disabled且执行已排空；其余运行状态返回STATE_CONFLICT |
| 生效 | 更新generation/revision、旧准备句柄失效，返回更新后的部署及ETag |
| 不可变定义 | 原limits、微版本、父绑定及已有Skill定义不变 |

出厂关闭联网，必须经“停用 → 等待disabled且排空 → 手动设置network_enabled=true → 重新启用”后才能搜索。HttpBinding.enabled、微部署enabled与network_enabled分别控制，不互相代开。

Prepare/Ready允许network_enabled=false；搜索Invoke若开关为false，在worker创建及任何发送前返回PERMISSION_DENIED（violations.field=network_enabled），不消耗sequence、不fallback，不返回SEARCH_NO_PROVIDER。true仍须controlled-http-v1、当前HTTP权限、获批固定绑定和平台egress共同允许；不提供任意fetch/socket。

每次ProxyHttp重新检查network_enabled、enabled及generation。停用或配置不能把已发送的未知结果改成未发送；历史Call和计费证据仍按原状态核实。

## 每个用户的服务商绑定

HttpBinding由核心CRUD接口管理，绑定到具体MicroDeployment，并从可信认证上下文确定归属。用户可以只配置一个服务商，也可以配置声明过的有序端点别名；无可用绑定时返回`SEARCH_NO_PROVIDER`，不使用平台隐藏的公共Key。

| 字段 | 约束与作用 |
|---|---|
| `endpoint_alias` | 引用该微版本声明的`brave`或`tavily`，同微部署内唯一；脚本以此alias选择固定端点 |
| `priority` | 整数0..1000，0最优；越小越先尝试 |
| `enabled` | 默认`false`；只允许启用绑定进入新调用的候选快照 |
| `secret_id` | 核心托管Secret引用；代理使用，投影不返回此字段 |
| `timeout_ms` | 整数100..4000，默认4000；还受根Call剩余期限限制 |

以上为HttpBindingSpec字段。核心外层记录提供不可变绑定UUID、实际微部署归属和revision，不能从脚本业务输入覆盖。创建、修改、禁用和删除遵守核心幂等键、乐观锁、归属与审计规则。

| 核心路径 | 操作 | 权限 |
|---|---|---|
| `/api/platform/v1/micro-deployments/{id}/http-bindings` | 列出 / 创建绑定 | `micro.http.read` / `micro.http.bind` |
| `/api/platform/v1/micro-deployments/{id}/http-bindings/{binding_id}` | 读取 / 修改 / 删除绑定 | `micro.http.read` / `micro.http.bind` |

权限作用于真实微部署，不能用另一个微部署的binding_id替换。读取配置不回显密钥内容；创建或换绑Secret还需`secret.bind`。精确HTTP方法、请求响应模型与状态码以[核心OpenAPI](../../contracts/openapi.json)为准。

只拥有微部署管理权不隐含使用任意Secret；绑定凭证还需当前操作者具有相应密钥绑定权。普通Runtime SecretBinding的明文租约不用于本预设，脚本和MPR均不能调用LeaseSecret取得搜索凭证。

## 优先级与快照

调用开始时，核心鉴权并冻结符合当前授权的启用绑定；按`priority`升序、同值按UUID稳定升序排列。脚本按固定序列执行最多三个attempt，同一alias最多一次；首版只声明两个alias，实际最多两个attempt。不能在执行期间重新发现新绑定补位。

核心向VM提供`host.httpBindings()`的非敏感投影，每项只含`alias`、`priority`、`timeout_ms`、`binding_id`；数组顺序即冻结顺序，alias固定为brave或tavily。投影不包含Secret引用或内容、认证头、委托、工作负载证书、内部网络地址和可复用授权票据。

冻结后添加绑定、改变优先级或超时只影响新Call，不改写当前调用的候选序列。冻结不是永久授权：每次转发仍检查源权限链、微部署、Runtime和原绑定；禁用、删除绑定或撤销Secret/微权限必须立即阻止尚未发送的attempt。

脚本不能通过伪造alias、seq、input中的owner或provider等字段切换到快照之外的绑定。用户、子账户和API Key使用具体绑定需显式有效权限；账户祖先关系不自动共享第三方凭证。

## 默认建议

| 配置 | 默认 / 硬边界 |
|---|---|
| 服务商启用 | 配置并审批前无服务商可用 |
| 候选顺序 | 用户显式配置priority，数值越小越优先 |
| 单绑定超时 | 4000ms，允许100..4000ms |
| 总调用期限 | 15000ms，包含Prepare、排队、所有HTTP和结果校验 |
| HTTP总attempt数 | 最多3，串行；同alias最多1次 |
| 微部署活动并发 | 模板默认1，硬上限4，同时受父MPR总4限制 |
| resources.cpu_millis / memory_bytes | 默认250 / 268435456，全部该微部署worker合计共享硬上限 |
| network_enabled | 默认false，须在停用、disabled且排空后手动打开 |
| heap / stack | 16MiB / 1MiB |
| 输入 / 输出 / stderr | 各1MiB / 1MiB / 64KiB |
| 单attempt请求 / 响应 | 64KiB / 解压后512KiB |
| 重定向 | 不允许 |
| 搜索结果缓存 | 本预设v1不定义独立结果缓存；不复用MPR制品缓存 |

第三方价格、配额和速率限制由服务商账户决定，不能把本地attempt上限当作人民币或其他货币的精确费用上限。返回的usage统计外部请求数量和可能计费数量，不作账单承诺。

父MPR4000mCPU/4294967296字节（4vCPU/4GiB）的工作池为3500mCPU/3758096384字节（3584MiB），另有控制CPU500m、控制内存128MiB及缓存256MiB。默认最多4个不同微部署活跃；总活动槽4只是上限，资源不足须等待或拒绝，不借用内核4C/16GiB预留。

## 日志与可见性

默认日志和审计只记录Call、微部署、绑定修订、服务商、尝试序号、状态及耗时，不写query、上游正文、认证头或Secret。诊断内容仍受原Call读取权和微诊断权限约束。

本预设不添加iframe页面或桌面组件。WebUI可用统一Skill调用界面和异步任务中心展示已授权输入输出；搜索结果摘要作为普通文本显示，返回URL不会被平台自动抓取。
