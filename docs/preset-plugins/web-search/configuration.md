# 网页搜索配置与HttpBinding

[网页搜索目录](README.md) · [服务商适配器](providers.md) · [执行协议](execution.md) · [密钥托管](../../runtime/secrets.md)

> 规格版本：2.6.0；状态：待实现的规范性配置设计。

## 微版本配置

本预设除HttpBinding外不定义额外普通配置；`defaults.json`为`{}`，由`defaults.schema.json`约束为空对象，不向MicroManifest添加未声明的config_schema字段。API Key、服务商列表、目标URL和认证头不能放进微版本配置、Skill业务输入、源码常量或普通部署环境变量。

不可变MicroManifest声明`execution_profile=controlled-http-v1`及`brave`、`tavily`两项`http_endpoints`。修改方法、origin、path、认证位置或payload_schema必须发布新微版本，并按原流程创建新的微部署和候选Skill；HttpBinding不能修改这些制品内容。

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
| heap / stack | 16MiB / 1MiB |
| 输入 / 输出 / stderr | 各1MiB / 1MiB / 64KiB |
| 单attempt请求 / 响应 | 64KiB / 解压后512KiB |
| 重定向 | 不允许 |
| 搜索结果缓存 | 本预设v1不定义独立结果缓存；不复用MPR制品缓存 |

第三方价格、配额和速率限制由服务商账户决定，不能把本地attempt上限当作人民币或其他货币的精确费用上限。返回的usage统计外部请求数量和可能计费数量，不作账单承诺。

## 日志与可见性

默认日志和审计只记录Call、微部署、绑定修订、服务商、尝试序号、状态及耗时，不写query、上游正文、认证头或Secret。诊断内容仍受原Call读取权和微诊断权限约束。

本预设不添加iframe页面或桌面组件。WebUI可用统一Skill调用界面和异步任务中心展示已授权输入输出；搜索结果摘要作为普通文本显示，返回URL不会被平台自动抓取。
