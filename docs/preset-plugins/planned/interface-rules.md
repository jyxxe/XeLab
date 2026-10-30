# 规划能力的共享接口规则

[规划总览](README.md) · [Skill规划](../../skills/planned/README.md) · [错误协议](../../communication/errors.md)

> 规格版本：2.8.0；状态：planned；业务契约草案：0.1.0。

## 输入与输出

能力接收结构化 JSON，拒绝接口未声明的字段。可省略字段由固定处理器在结构验证后按约定应用缺省值，Schema的default注解不会自动注入字段；可省略不等于允许显式 `null`。普通插件默认 JSON 输入≤4MiB、输出≤16MiB、深度≤64；微插件默认完整输入/输出各≤1MiB、深度≤64，还受 MPR stdout/堆/CPU限额约束，先到者生效。

Schema 的 `maxLength` 计 Unicode 码点，整体封装按 UTF-8 字节计量。业务若另外限制字段字节数，两个上限同时检查。JSON数必须有限且可按 I-JSON 精确表达；大整数、定点数和二进制使用接口指定的十进制字符串或规范base64，不静默截断、猜类型或替换编码。

声明文件输入的普通能力由获授权的核心 Blob 传递，单个≤64MiB。Blob ID、文件名和 MIME 声明均不授予文件权限；拒绝原始宿主路径。生成结果 Blob 要检查当前主体权限、存储限额与失效时间；有结果账本的重复请求复用同一结果，过期后不能假装原引用仍可读。当前pure微插件只接收JSON文本、base64或声明式文件描述，没有Blob/宿主文件读取权；未来可信文件桥须另行满足对应gate。

JSON Pointer采用RFC6901字符串形式：空串为根，`~0/~1`分别转义`~/`，不解释URL片段、表达式或代码。只访问当前JSON值的自有成员；数组下标为规范非负十进制，越界视为缺失。目标为对象路径的插件拒绝重复或祖先重叠路径与危险原型键。

## 身份、依赖与数据边界

所有业务经核心 SkillRouter 转发，调用主体、祖先链、根调用deadline、固定目标版本和幂等键由核心上下文提供。业务输入不得指定主体、源码、运行时地址、secret值或任意目标插件来替换绑定。

普通能力同时检查 `skill.invoke`、对应 `capability.invoke` 及目标资源；微能力同时检查 `skill.invoke`、`micro.invoke`、`micro.runtime.use` 和固定部署。授权遵循现有Guest/权限组/子账户/API Key边界；权限组和父级授予不能扩大派生主体的当前权限上限。

`connection_alias/transport_alias/route_alias/model_alias/policy_alias/repository_alias/browser_alias` 等只是经管理员或具备对应管理权限的主体批准的配置索引。执行前校验别名ACL与当前撤权；不能把“知道别名”当作使用许可。平台密钥管理、网络策略、Blob、事件接口是依赖，不自动授予插件；缺少必要批准即禁用对应功能。

依赖中的库必须锁定版本、来源、许可证及制品摘要并纳入资源预算；“纯JSON支持”不表示可以加载宿主模块。普通插件间复用HTTP、邮件或文件服务时必须声明固定能力依赖及委托，不得使用任意插件ID或转发调用者全部权限。

## 微部署资源与联网设置

MPR 1.2.0中，每微部署默认`resources.cpu_millis=250`、`resources.memory_bytes=268435456`（256MiB），可在获批范围内手动配置。核心适配器配置逐微cgroup，对该微部署全部worker实施合计CPU/内存硬限制；并发默认1、硬上限4，共享同一资源预算。JS heap默认16MiB仍是每VM子上限，不替代部署的合计内存限制；无需每微OCI或MPR管理宿主cgroup。

父MPR默认为4000mCPU/4294967296字节（4vCPU/4GiB）：微工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、制品缓存256MiB，内存合计3968MiB。工作池按默认250mCPU/256MiB算术容量为14个微部署，但先受4个硬活动槽限制，默认最多4个不同微部署同时活跃；同一微部署增加worker也不复制资源。资源先从父工作池准入，不能借用内核至少4C/16GiB预留。

| 设置接口 | 约束 |
|---|---|
| 方法 / 路径 | `PUT /api/platform/v1/micro-deployments/{id}/settings` |
| 请求正文 | `{resources:{cpu_millis,memory_bytes},network_enabled}`，完整替换这组设置；字段类型及获批范围以核心模型为准 |
| 并发与幂等 | 必须提供当前`If-Match`和`Idempotency-Key`，并通过当前部署管理权限及来源链校验 |
| 可写状态 | 仅`enabled=false && phase=disabled`且活动执行已排空；状态不满足返回`STATE_CONFLICT` |
| 生效 | 返回更新后的部署及ETag，更新generation/revision、使旧准备句柄失效 |
| 固定定义 | 原limits、微版本和父绑定不变，已有候选及注册Skill不漂移 |

管理流程为“停用 → 等待disabled及排空 → PUT settings → 重新启用”；UI使用相同流程，不提供执行中热改。历史Call及unknown证据不因设置变更或代次更新而重写。

`network_enabled`默认false。Prepare/Ready允许false，pure-json-v1仍可正常执行；controlled-http-v1业务Invoke若为false，在创建worker及任何发送前返回`PERMISSION_DENIED`，`violations.field=network_enabled`，不占sequence、不fallback，也不伪装成无服务商。

true仅打开受控HTTP的必要前提，仍须固定controlled-http-v1、当前权限、获批HttpBinding及父egress/平台策略；不提供任意fetch/socket，不使pure脚本取得HTTP桥。每次ProxyHttp重新检查开关、enabled及generation，不能用旧句柄或旧快照绕过当前设置。

## 调用、批量与副作用

单次接口是有界基本操作。多项处理使用核心异步batch，提交后查询进度及逐项结果；单项数据量仍受本能力和核心封装的共同限制，不提供整批事务。`flow.map/filter/condition/template`只返回数据，不自动生成任务、执行分支或调用其他插件。

`none` 表示无外部业务写入；同一固定版本及输入可在当前授权与剩余deadline内有界重试。外部HTTP、模型计费、发送邮件/通知、数据库写入和浏览器导航保守标记 `non_repeatable`；不能因为HTTP方法是GET或存在业务dedupe_key就自动认定可重放。

`idempotent` 用于需要持久幂等账本的事件提交、归档输出、仓库文件输出等。账本键至少绑定调用主体、固定版本、规范输入摘要及核心幂等键；冲突输入拒绝。发布结果前落盘，重复请求复用结果；不能只靠进程内缓存防重。

取消、deadline、断线或崩溃不撤销已经发生的外部副作用。外部接受状态不明时保持结果未知，先查询可靠证据或由授权用户核实，禁止盲目切换渠道/服务商重发。只有可靠证明尚未发送或上游正式支持相同幂等键时才重试；总attempt和总deadline不重置。

`ai.chat` 的业务流复用核心流协议，保持顺序、背压和取消；开始发送内容后不自动切换模型或从头重播。通知/邮件的 `accepted` 表示上游已接受，不是本地排队；完成的Call结果不会以后自动变成delivered。持久后台投递与回执需另行设计状态接口。

## 错误与验收

复用当前核心错误信封；格式/条件/表达式不合法为输入错误，超预算为资源错误，未授权/撤权为权限错误，依赖或指定模式未提供为不支持/不可用错误。插件不能通过stdout伪造核心授权错误或成功后再提交失败帧；微插件业务错误须遵守运行时声明及映射。

无匹配结果允许返回完整空集合；若结果、文件、列或组数超限，除接口明确声明 `truncated` 的只读操作外，整项失败并清理未发布输出，不返回看似完整的部分成功。计数、索引、字段集合与排序顺序必须与返回内容一致。

结构样例仅检查已声明的字段、分支及限额；发布还必须通过每个插件的算法、授权撤销、资源耗尽、故障恢复和依赖失效验收。与外部系统交互的实现应使用可控测试端点核实证据与重试行为，不能用静态Schema测试代替。
