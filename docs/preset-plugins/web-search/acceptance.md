# 网页搜索验收条件

[网页搜索目录](README.md) · [执行协议](execution.md) · [Skill定义](../../skills/web-search/README.md)

> 规格版本：2.8.0；状态：待实现的验收规范。以下条目是未来实现必须证明的行为，不是已经通过的测试报告。

## 制品与核心对象

- 目录项`org.xelab.web-search`的kind为`micro`，没有独立OCI镜像、容器、数据卷和静态业务Skill。
- 微版本固定javascript、json-stdio-v1、controlled-http-v1与non_repeatable，config仅接受`{}`。
- 两个http_endpoints的方法、origin、path、认证位置及payload_schema随package_digest固定；修改必须产生新制品。
- 发布版本不产生Skill；创建MicroDeployment后核心才生成绑定该部署的candidate_skill，初始不自动启用或注册。
- 候选Skill固定微版本、摘要、微部署和真实Runtime安装；不能用同名latest或其他Runtime静默替换。
- 父MPR不支持该profile或未获批时，不得把微部署报告为可执行；不降级成纯计算执行成功。
- MPR版本及协议要求1.2.0；settings更新不修改原limits、微版本、父绑定或候选/注册Skill定义，generation/revision更新使旧句柄失效。

## 用户与绑定

- 不同用户的绑定、Secret引用、配置、Call和诊断按可信归属隔离，输入自报owner无效。
- 只授予一个绑定的子账户/API Key不能调用同部署其他绑定，源权限链撤销在下一次发送前生效。
- 所有绑定禁用或缺失时返回SEARCH_NO_PROVIDER，不偷用平台公共凭证；同微部署的endpoint_alias唯一。
- priority为0..1000，数值越小越优先；相同priority的UUID排序稳定；每Call冻结启用绑定的有序snapshot并最多尝试三个alias。
- 快照冻结后改序、添加绑定或变更timeout，不改变在途候选；绑定和Secret撤销仍阻止后续发送。
- 用户配置不能更改目标URL、headers或payload_schema；脚本无法读取Secret内容或复用工作负载委托。
- 配置读取检查micro.http.read，变更检查micro.http.bind，创建/换绑Secret另检查secret.bind；新绑定默认disabled。

## 网络与输入

- 微部署出厂network_enabled=false；Prepare/Ready仍允许，但搜索Invoke须在worker创建和发送前返回PERMISSION_DENIED，violations.field=network_enabled，不消耗sequence、不fallback、不混为SEARCH_NO_PROVIDER。
- 手动开启须先停用、phase=disabled且完全排空，PUT settings携If-Match及Idempotency-Key；其余运行状态返回STATE_CONFLICT，配置后重新启用。
- 开关true仍需controlled-http-v1、当前HTTP权限、固定HttpBinding和平台策略；每次ProxyHttp复核开关、enabled及generation，旧快照不能绕过撤销。
- query按Unicode code point计量1..600，按空白分隔的词最多75；空串、全空白和超限拒绝，limit只接受1..20整数。
- 不允许附加provider、owner、headers、url、credential等输入字段；缺省limit的实际值为10。
- 任意URL、别名越界、非法payload、未声明端点均在发送前被拒绝，拒绝不会触发fallback。
- 固定HTTPS证书验证、每连接DNS与目标IP检查能阻断回环、宿主、内部地址和元数据目标。
- 对301/302/307/308不跟随；返回结果URL、Location和callback URL不会触发第二次抓取。
- 每attempt请求64KiB、解压后响应512KiB的边界真实生效，压缩响应不能绕过计量。
- 普通微profile不能访问HTTP桥；受控profile仍不能使用fetch、socket、filesystem、process和普通Node模块。

## 服务商与输出

- Brave只访问声明的GET端点，并从web.results映射标题、URL和摘要；不返回其他结果分区。
- Tavily只访问声明的POST端点，固定basic/general及关闭auto_parameters、答案、原文和图片。
- Tavily查询字段不能覆盖常量或自动升级到advanced；认证头仅由代理注入。
- 输出只有query、provider、items、count、fallback_used、attempts及usage，items顺序保持上游顺序且不超过limit。
- count与items长度一致；provider仅brave或tavily；URL是无凭证的HTTP/HTTPS绝对URL。
- 合法空结果是当前provider的成功，不触发fallback；非法响应不能伪装成空结果成功。
- 输出不包含原始错误正文、原始headers、原始凭证、输入中的未声明字段或服务商生成答案。
- 代理不向脚本返回非2xx错误正文；成功正文若echo Secret原值，核心拒绝暴露，不能触发普通fallback。
- HTTP桥返回Promise，事件泵可在有界预算内处理完成；仅主Promise fulfilled且无pendingHostCall时才能校验成功输出。

## fallback与计费恢复

- 三个alias按顺序串行尝试，同alias最多一次，总attempt最多3；HTTP库、Broker和监督器不叠加隐式重试。
- 发送前连接失败、429、5xx和明确上游认证拒绝可以fallback；平台权限失败、SSRF和输入拒绝立即停止。
- 其他明确失败按规范终止，候选明确失败用稳定SEARCH_PROVIDERS_EXHAUSTED；invalid_response用SEARCH_RESPONSE_INVALID。
- 请求已发送后超时或连接中断且无法核实结果时为UPSTREAM_OUTCOME_UNKNOWN，不再尝试下一alias。
- unknown同时锁定核心Call，恶意脚本继续调用另一个alias也不能产生外部请求。
- 业务错误经已声明的host.fail映射为失败，不能写成成功JSON；unknown优先于脚本catch、host.fail或正常stdout。
- 相同Call/seq重复代理调用不重复收费请求；相同seq而不同payload被拒绝，崩溃恢复不重发未知请求。
- 已知not_sent不增加external_requests；可能发送的请求保守计入potentially_billed_requests，不按429/5xx宣称免费。
- 全部attempt和根Call共享原15000ms总期限，每绑定最多4000ms；排队、重Prepare和fallback不能重置期限。
- 取消会停止新发送并释放本地资源；不能把已发送请求承诺为未执行或未收费。

## 资源、日志与异步调用

- 单微部署默认resources.cpu_millis=250、resources.memory_bytes=268435456；核心适配器为其逐微cgroup配置合计CPU/内存硬上限，全部worker共享，默认并发1/硬上限4；不创建每微OCI，不向MPR授予宿主cgroup权限。
- 父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB）：工作池3500mCPU/3758096384字节（3584MiB），控制CPU500m及控制内存128MiB，缓存256MiB。默认最多4个不同微部署同时活跃，总活动槽4只是上限；不足时等待/拒绝，不借内核4C/16GiB预留。
- 输入/输出各1MiB、heap16MiB、stack1MiB和stderr64KiB限制真实生效；日志截断带标记。
- 搜索输出和查询不得进入MPR代码制品缓存，跨用户不能读到其他Call结果或推测其Secret。
- 默认日志不含query、上游正文、Secret或认证头；微诊断和Call结果读取沿用当前权限。
- 异步batch通过固定Skill运行，unknown item不会自动重放；返回成功结果和错误均不泄漏其他item数据。
- 文档所示资源、吞吐和上游可用性仅在实际压测后才能升级为实测结论。
