# 网页搜索执行与恢复

[网页搜索目录](README.md) · [配置](configuration.md) · [适配器](providers.md) · [核心恢复](../../tasks/recovery.md)

> 规格版本：2.6.0；状态：待实现的规范性执行协议。

## 固定调用上下文

核心验证固定Skill、微版本摘要、微部署、父Runtime、当前来源链以及具体HTTP绑定的使用权限。调用者只提供query和limit；正文中的主体、Secret、provider、网络地址或运行时ID不能替代可信控制上下文。

输入通过Schema和额外词数校验后，核心冻结当前启用绑定形成当前Call的有序snapshot，并随可信执行封装交给MPR。脚本最多尝试其中三个不同alias。Prepare句柄仍固定原微版本、package_digest和运行时/微部署代次；不能因故障改用同名latest版本。

每Call使用新隔离VM，虚拟stdin收到一份完整JSON，stdout只能输出一份完整JSON。`controlled-http-v1`只新增下列受控桥，普通Node模块、fetch、socket、文件系统、process和直接HostService访问仍不可用。

| 桥 | 语义 |
|---|---|
| `host.httpBindings()` | 同步返回有序数组；每项仅alias、priority、timeout_ms、binding_id，不重新发现其他用户配置 |
| `host.httpRequest(alias,payload,seq)` | 返回Promise；请求当前snapshot内的alias，payload必须匹配不可变端点Schema，seq标识本Call内的唯一attempt |
| `host.fail(code,message)` | 报告MicroManifest.business_errors声明的业务失败，由Runtime映射为MicroInvokeResponse.error，不把错误写成成功JSON |

MPR监督器把脚本请求转换为受信`HostService.ProxyHttp`调用。核心根据实际MPR实例、Call委托、固定微部署和snapshot确认授权；脚本不能自行构造控制身份。桥不返回原始Secret、认证头或可复用委托。

脚本以async IIFE作为最外层执行表达式，逐次await受控HTTP Promise；VM不得同步阻塞MPR监督器。MPR事件泵在有界预算内等待完成，仅当主Promise fulfilled且没有pendingHostCall时，才把完整stdout送入成功输出校验。主Promise拒绝、提前结束后仍有未完成HTTP或输出不合法均不能报告业务成功。

代理结果由下列受控字段组成，脚本不能把非2xx错误正文当作响应继续解释：

| 字段 | 定义 |
|---|---|
| `outcome` | `completed`、`not_sent`或`unknown` |
| `status_code` | 有效HTTP状态整数，未取得时为null |
| `body_json` | 仅2xx且已通过代理检查的JSON正文；否则为空值 |
| `error_code` | 规范化错误码，无错误时为null |
| `retry_after_ms` | 可用且合法的上游重试提示，否则为null；不授予同alias重试权 |
| `duration_ms` | 该代理操作的耗时 |
| `operation_id` | 核心记录的本次代理操作标识 |

`unknown`或对应未知结果错误码必须立即停止脚本fallback。核心同时锁定该Call，拒绝后续HTTP操作，不能依赖脚本自律防止继续发送。

## 正常顺序

1. 若无候选绑定，通过host.fail报告`SEARCH_NO_PROVIDER`，不创建外部请求。
2. 取snapshot下一alias，按其provider构造白名单payload，创建唯一attempt记录。
3. 核心重新检查绑定、Secret与来源链是否撤销，并以根Call剩余时间和绑定timeout_ms的较小值作为期限。
4. 先持久记录即将发送的attempt，再执行最多一次上游请求；不跟随重定向，不做HTTP库隐式重试。
5. 成功响应通过大小、JSON和输出校验后，返回统一结果。即使items为空也直接成功。
6. 仅对下表允许的确定失败进入下一个alias；没有剩余候选或预算时返回稳定错误。

每次最多一个上游请求在途，总attempt最多3，同alias最多1次；首版声明的两个alias实际最多产生两个attempt。不存在并发竞速、对同一provider的额外重试、动态补位或跨provider结果合并。

## fallback决策

| 结果 | attempt outcome | 行为 |
|---|---|---|
| 请求字节发送前的连接失败 | `not_sent` | 若有下一候选和剩余预算，fallback |
| 上游HTTP 429 | `rate_limited` | fallback；不在本Call等待后重试同alias |
| 上游HTTP 5xx | `upstream_error` | fallback |
| 上游明确拒绝凭证，如401/403 | `auth_rejected` | fallback；不修改Secret或降级平台权限 |
| 合法成功JSON，含空列表 | `success` | 结束，返回当前provider结果 |
| 其他明确上游失败 | `upstream_error` | 终止并返回`SEARCH_PROVIDERS_EXHAUSTED` |
| 非法JSON、超大或不匹配响应 | `invalid_response` | 终止并返回`SEARCH_RESPONSE_INVALID` |
| 已发送后超时、断连或结果不能核实 | 不构造普通成功输出 | `UPSTREAM_OUTCOME_UNKNOWN`，停止全部fallback |
| 平台鉴权/来源链/撤权/SSRF或输入拒绝 | 不伪装为服务商失败 | 保留对应平台错误，立即终止 |

HTTP 401/403只表示固定外部服务商的响应时，才适用auth_rejected；核心或MPR的权限拒绝不能按此规则跳过。上游错误正文中的任意文字不授予重试权。

## 期限与计量

总期限15000ms从根Call开始计时，包含核心排队、Prepare、MPR排队、编译、桥等待、所有网络和结果校验。每attempt最多4000ms；新的attempt、重Prepare或fallback不重新获得完整总期限。

输入和输出各最多1MiB、深度64；JS heap 16MiB、stack 1MiB、stderr最多64KiB。每attempt请求最多64KiB，响应按解压后512KiB计量。达到硬期限时监督器取消代理和执行单元，并释放活动槽。

`usage.external_requests`统计已经可能发出请求字节的attempt数；确定未发送的连接失败不增加。`usage.potentially_billed_requests`保守统计其中可能收费的请求，不能据状态码推定免费。v1在不能确认不收费时将其计入，数值不高于external_requests。

成功输出的`attempts`保留顺序和规范化outcome，`http_status`在已得到有效HTTP状态时填写，否则为null。`fallback_used`表示是否进入过初始alias之后的候选attempt，不因空结果而置true。

## 非重复执行与unknown

该微版本声明`side_effect=non_repeatable`。虽然搜索不修改平台知识数据，上游仍可能逐请求收费，不能把查询当作可以任意重算的纯计算。

核心持久attempt台账绑定Call、snapshot、seq和请求摘要。相同seq的重复ProxyHttp必须返回已有确定结果或未知状态；不能重发。相同seq但不同alias或payload属于冲突，拒绝执行。已发送记录不明时不得通过更换seq补发同alias。

核心、Broker、MPR监督器和HTTP客户端均不能在连接重置、崩溃或超时后叠加自动重试。根Call的结果查询只核实已有执行状态；v1不定义ambiguous fallback开关，也不自动查询另一个服务商来掩盖未知收费。

若仍能证明请求未发送，可按正常fallback进入下一绑定；若代理已经开始可能发送而无法证明结果，使用`UPSTREAM_OUTCOME_UNKNOWN`，Call与所属batch item进入unknown。它不能混成可重试的普通失败或`SEARCH_PROVIDERS_EXHAUSTED`。

## 取消、撤销和批量

取消传播到脚本、监督器和代理。尚未发送的请求不得继续发送；已发送的外部请求可能继续处理和收费。结果未能核实时仍保留未知状态，不能因为本地取消已确认就承诺没有外部费用。

snapshot配置变更不改变当前顺序和参数，但下一attempt发送前必须重新检查Secret、绑定、微部署与权限撤销。禁用或删除绑定同样立即阻止新发送；迟到响应不能创建新attempt或覆盖已终结状态。

多条查询使用[核心异步batch](../../skills/batches.md)，各item引用同一固定Skill和一份业务输入。每项独立计量和诊断；unknown项不能由BatchEngine自动重放，其他已完成item不受其影响。

无配置使用`SEARCH_NO_PROVIDER`；候选明确失败使用`SEARCH_PROVIDERS_EXHAUSTED`；无效响应使用`SEARCH_RESPONSE_INVALID`。错误通过声明过的host.fail报告并遵守核心ErrorDetail，不附原始上游错误正文、响应headers或Secret；详细attempt信息按原Call授权读取。

业务错误只允许MicroManifest.business_errors列出的代码，不得伪造平台保留码、401或403。根Call出现外部结果unknown后，核心以`UPSTREAM_OUTCOME_UNKNOWN`和HTTP409为准；即使脚本catch、host.fail或输出正常JSON，也不能覆盖unknown或再次发送。
