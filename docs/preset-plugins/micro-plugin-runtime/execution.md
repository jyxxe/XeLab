# json-stdio-v1执行协议

[MPR目录](README.md) · [安全边界](security.md) · [接口](interfaces.md)

> 规格版本：2.8.0；状态：待实现。engine固定为javascript；stdin/stdout/stderr是虚拟缓冲，不是宿主文件描述符授权。

## 一次请求，一份新状态

核心按固定微版本和package_digest向已绑定MPR准备制品，再发起调用。MPR核对可信请求、输入Schema和限额；核心完成父/微部署资源准入、验证worker进程并应用子cgroup后，才允许短命worker创建新VM执行JS。不得复用上一次请求的可变全局状态。

监督器先创建停等且无用户代码的worker，经HostService.AttachMicroWorker提交当前Call委托、核心group句柄、容器PID/启动时间。核心验证namespace/start-time/pidfd、Call和父实例代次，迁入准确子组并Ack后才允许运行；失败终止回收，启动内存/CPU仍受control预算约束。

代码只从已批准不可变制品加载。请求不接受即时source、宿主路径、命令、argv、env或任意解释器选择。Prepare只准备/核验制品，业务代码在受限Invoke执行阶段求值，不能在无调用额度的缓存准备阶段提前运行。

MicroManifest.execution_profile缺省为pure-json-v1，必须side_effect=none且http_endpoints缺省/空，默认2s。controlled-http-v1必须side_effect=non_repeatable并声明1..8个固定endpoint，整次wall最多15s。省略新字段的旧pure制品按原字节与摘要解释，不能为补默认值重算包/源码摘要。

MicroDeployment.network_enabled默认false。Prepare/Ready可以完成，pure照常调用；controlled-http业务Invoke在关闭状态于创建worker前返回PERMISSION_DENIED，violations.field=network_enabled，不消耗HTTP序号也不走服务商fallback。手动启用仍不免除固定endpoint、当前权限、binding/secret和出口检查。

微部署resources默认memory_bytes=268435456、cpu_millis=250，同组所有活动worker共享，不按每Call复制。CPU节流和HTTP等待均计wall期限；单次heap/stack/JSON等原limits继续生效。进程地址空间1GiB仅是虚拟空间保护，不是内存配额或可用RSS保证。

## 虚拟I/O

| API | 输入与结果 | 定义 |
|---|---|---|
| stdin.readAll() | 无参数 → string | 读取本次输入缓冲的完整UTF-8 JSON文本，到EOF结束 |
| stdout.write(text) | string → 无业务结果 | 将字符串追加到本次输出缓冲，逐次检查UTF-8字节预算 |
| stderr.write(text) | string → 无业务结果 | 追加受限诊断文本，与stdout分离，不作为返回值解析 |

输入是一份完整JSON值，其类型由该微版本input_schema约束。输入在执行前由核心与MPR分别验证，最多1MiB UTF-8字节、嵌套深度64，拒绝非法UTF-8、重复key、非有限数字和不符合Schema的内容；不把输入字符串拼成JavaScript代码。

stdout可以分次write，但正常完成后拼接结果必须恰好包含一个合法JSON值和可选前后空白，最多1MiB UTF-8字节、嵌套深度64；输出同样拒绝重复key和非有限数字。多值、空输出、额外调试文字、非法JSON或违反output_schema均失败，不返回部分结果。脚本求值的返回值不自动代替stdout。

一个最小处理器可采用以下形式：

```javascript
const value = JSON.parse(stdin.readAll());
stdout.write(JSON.stringify({text: value.text}));
```

代码示例应与其固定input/output Schema一同发布。微插件声明的Schema、资源上限和代码摘要共同构成版本；只改代码或Schema也需要新版本，不能保留旧UUID却换掉处理器。

## 完成与失败

只有正常完成、未取消、未越限且输出校验通过，才可提交成功结果。未捕获JavaScript异常、内存/栈越限、硬wall超时、被监督器终止或协议错误均为失败；不能因为stdout提前写出了一段合法JSON就忽略后续异常。

输出超过上限立即终止，不截断为一个貌似成功的JSON对象。stderr超过64KiB收集上限后截断并标明stderr_truncated，不单因日志截断判业务失败，仍执行CPU/wall约束；大量写日志不能阻塞监督器或扩大内存。

日志不得混入stdout或返回内部路径、凭证和宿主堆栈。用户脚本可能把输入写进stderr，日志可见性因此按该微插件和调用主体的权限控制，不能默认公开给其他用户。

## 取消、结果核实与batch

核心取消传播到当前VM/worker，先停止新工作，再中断或按稳定PID身份终止本次执行单元。迟到输出不能覆盖已取消/已终结Call。单Call取消只影响该请求；微部署cgroup OOM会按组结束该部署相关worker，核心逐Call区分失败与已发HTTP的unknown，不把同组资源故障误报为其他用户代码都成功。

pure-json-v1为无外部副作用的JSON计算。核心保存权威Call状态与结果，Cancel/QueryResult复用现有能力控制协议；MPR重启丢失内存不等于核心已保存结果消失。pure结果证据缺失时按原固定版本、原幂等关联及side_effect=none规则处理；controlled-http-v1按下述持久代理台账处理，不自动改脚本或改Runtime安装。

批量调用使用核心异步batch，每个item仍执行一个完整微版本请求。batch返回202后通过核心查询进度/逐项结果；关闭页面或淘汰制品缓存不取消已接受批次，也不放宽单次限额。

微输入/输出Schema采用[MicroBusinessSchema](../../api/models/MicroBusinessSchema.md)受限关键字集合，每份最多64KiB、256节点、深度16；拒绝$ref循环、远程引用、任意正则、自定义format及资源授权标记。核心和执行端均限制JSON解析深度64、节点/求值预算与根deadline，Schema评估超预算明确失败，不在核心执行用户代码。

脚本失败时运行时可在MicroInvokeResponse的error分支携带有界诊断，核心分别保存错误及受限诊断；鉴权失败不回显私有日志，传输超时/崩溃可能没有诊断。

## controlled-http-v1虚拟桥

| API | 输入/返回 | 契约 |
|---|---|---|
| host.httpBindings() | 无参数 → 有序数组 | 本Call冻结绑定的alias/priority/timeout_ms投影，按priority、UUID排序；不含secret ID/值或可覆盖的地址 |
| host.httpRequest(alias,payload,seq) | 固定alias、JSON payload、1..3 sequence → Promise<MicroProxyResult> | 监督器通过核心ProxyHttp执行固定endpoint；最多1个在途请求，不接受URL、header或secret |
| host.fail(code,message) | 声明的业务code、有界message → 终结业务错误 | code只能来自MicroManifest.business_errors，由监督器产生MicroInvokeResponse.error，不增加宿主权限 |

HTTP脚本用async IIFE返回的主Promise组织顺序；受控VM事件泵等待该Promise和host回调，等待HTTP计入15s总wall但不CPU忙等。主Promise结束且无pending host调用后才校验stdout，禁止任意timer、fetch、socket或其他异步I/O；未处理拒绝/悬挂调用按异常或deadline失败。

endpoint的origin/path/method/auth_header/auth_scheme/payload_schema均来自固定微版本。GET payload仅为扁平string/int/bool对象，由核心固定编码器生成query；POST为I-JSON请求体，均先按payload_schema验证。请求上限64KiB；单次timeout取绑定timeout_ms、4000ms及整体剩余时间的最小值，响应最多512KiB。

MicroProxyResult包含operation_id、outcome(completed/not_sent/unknown)、status_code、body_json、error_code、retry_after_ms与duration_ms。status_code/body_json/error_code/retry_after_ms可空，duration_ms为0..4000；不返回headers。仅2xx返回有界body_json，非2xx原文丢弃；凭证直接回显为UPSTREAM_RESPONSE_INVALID，不把原响应交给脚本。

请求发出前由核心再次验证network_enabled、微部署enabled/原generation、当前微权限、绑定/secret有效性和egress；绑定快照不冻结撤权。配置内容变化不能让旧Call换key，priority更新只影响后续Call。核心从真实RpcDelegation推导调用身份，不信任VM自报call_id或主体。

## 单Call去重、fallback与unknown

核心持久化(call_id,seq)台账与请求指纹，指纹涵盖固定endpoint/绑定快照、alias及payload摘要。同seq同指纹返回原结果，异内容IDEMPOTENCY_CONFLICT；每alias最多一次真实请求，不通过新seq重复发送同一提供者请求。重启或丢失VM Promise不能清空已发请求证据。

脚本可在同一业务Call明确按绑定优先级尝试其他alias，但仅用于已证实的not_sent、上游429/5xx或上游认证拒绝。每次仍经过当前平台授权，最多3次、每次4s、共享15s总期限；不自动换attempt，也不把平台PERMISSION_DENIED/来源失效/STATE_CONFLICT视为可fallback的上游故障。

已发请求无法确认响应时，ProxyHttp.outcome=unknown，核心Call.state=unknown，后续任何seq被阻止。核心固定UPSTREAM_OUTCOME_UNKNOWN/409；脚本catch异常、调用host.fail、输出fallback文字或正常返回都不能把sticky unknown改成succeeded/failed。按已保存证据核实，不默认重跑non_repeatable脚本或换alias重新出网。

纯profile继续按side_effect=none规则核实/安全恢复。HTTP profile的取消、传输超时、MPR退出或batch恢复不得默认重发；只有确定未执行且经过显式原有恢复流程时才讨论后续操作，不能靠SDK自动重试制造重复外部调用。

## 声明业务失败

MicroManifest.business_errors可省略为空；每项固定code、http_status和retryable。http_status只允许400/409/413/429/502/503/504，禁止平台保留码和401/403；脚本不能任意选择HTTP状态或借业务错误冒充认证失败。

controlled VM的host.fail(code,message)只能使用该清单，监督器记录失败并产生MicroInvokeResponse.error，不能把失败对象塞进正常stdout伪装成成功调用。消息和诊断仍受隐私/长度约束；未知外部结果由核心台账优先处理，不受声明业务码覆盖。

## 手动设置与重新启用

联网开关和CPU/内存通过核心settings接口修改，必须先停用，等phase=disabled且无worker、Prepare或queued请求后用If-Match提交；原limits、源码/版本和父绑定保持不变。变更推进generation使旧句柄失效，再启用/Prepare；调用输入不能临时提高resources或打开网络。

停用立即写enabled=false/generation++撤路由并进入draining，完成取消与组回收才转disabled；清理失败保持draining/failed，不能热改settings。HTTP已发结果不明仍保留unknown，不以worker退出或新设置覆盖原Call证据。
