# 新增能力的Skill规划

[Skill目录](../README.md) · [32个插件规划](../../preset-plugins/planned/README.md)

> 规格版本：2.8.0；状态：planned。37个能力入口是待发布的业务契约，当前没有自动注册的Skill。

## 转发关系

| 类型 | 发布后的调用链 | 固定绑定 |
|---|---|---|
| 普通插件15个能力入口 | 调用者 → 核心SkillRouter → OCI插件能力接口 | 已签名插件、安装来源、能力契约名/版本/摘要和获授权部署 |
| 微插件22个能力入口 | 调用者 → 核心SkillRouter → MicroRuntimeService → MPR中的微制品 | MicroPlugin/Version/Deployment、包摘要、业务Schema摘要和运行时部署 |

逐接口字段在[普通插件目录](../../preset-plugins/planned/ordinary.md)和[微插件目录](../../preset-plugins/planned/micro.md)下的 `contracts/<operation>/README.md`。字段不重复抄为虚假的 `.skill.json`；正式注册时 `input_schema/output_schema` 必须与发布能力完全相同，并在目标中绑定正式契约摘要。

普通插件可声明多个能力，各能力分别生成或注册Skill。当前微制品每版本声明一个输入输出契约，因此 `micro-csv-json` 的双向转换、`micro-data-sort-group` 的排序/分组在发布时拆为各有MicroPlugin身份、版本、部署及候选Skill的独立微制品；不得将两个不同Schema塞入一个当前MicroManifest。规划清单中的plugin_id是逻辑目录标识，正式制品身份在发布时逐能力确定；目录不表示它们共用可变目标。

能力ID是发现标签，不是允许调用者输入的路由。Skill名称、调用源、目标和资源规则在正式发布时选择并冻结；不能把输入中的模型别名、URL、JSON表达式、数据路径当作任意Skill名或插件地址执行。

MPR 1.2.0的微部署默认resources为250mCPU/256MiB，所有worker共享核心适配器配置的逐微cgroup硬上限，默认并发1/硬上限4；每VM的16MiB JS heap仍为子上限。父MPR默认4000mCPU/4294967296字节（4vCPU/4GiB），其中工作池3500mCPU/3758096384字节（3584MiB）、控制CPU500m及控制内存128MiB、缓存256MiB，默认最多4个不同微部署活跃；总活动槽4不代表四份独立资源，不借用内核4C/16GiB。

资源及network_enabled由[微部署settings接口](../../preset-plugins/planned/interface-rules.md#微部署资源与联网设置)手动配置：仅停用、disabled且排空后，携If-Match和Idempotency-Key写入；更新generation/revision、失效旧句柄，原limits、微版本、父绑定与Skill定义不漂移。UI遵循停用、配置、启用流程，不在执行中热改。

## 调用、授权与批量

沿用[Skill双层授权](../authorization.md)与[微插件动态Skill](../micro-plugin-runtime/README.md)。普通插件检查Skill及capability授权；微插件还检查独立微对象与MPR使用权；调用者当前祖先链及派生API Key的权限上限始终生效。平台不因安装预设给Guest、子账户或微脚本添加默认权限。

微部署network_enabled默认false，pure计算及Prepare/Ready照常；controlled-http-v1业务Invoke在worker创建/发送前拒绝为PERMISSION_DENIED（violations.field=network_enabled），不消耗sequence或fallback。手动开启还需profile、当前权限、固定HttpBinding和平台策略共同允许；不授予任意fetch/socket，不使pure脚本取得HTTP桥，也不开放宿主cgroup管理。

所有多项调用优先通过[核心异步batch](../batches.md)，提交后查询状态与逐项结果。batch输入≤1000项/4MiB，单项还受各能力自己的JSON/CPU/文件限额；需要更多数据时分批提交，不隐藏在一个无限循环微脚本里。

纯JSON处理可在固定版本、剩余deadline及当前授权范围内重试。邮件/通知/SQL写入/模型计费/HTTP/浏览器导航等保守标记non_repeatable，未知结果须先核实，不因batch重启自动重发。归档/仓库文件输出及Webhook事件等按各能力的幂等账本恢复。

`trigger.webhook`由认证入口产生可信事件引用，再转为事件或任务；外部Webhook不能直接成为拥有调用者全部权限的Skill执行入口。`flow.*`只处理数据，和核心工作流调度之间的连接必须由独立授权的任务定义显式声明。

## 发布前核对

发布后的Skill才可被发现或启用；未满足[gate](../../preset-plugins/planned/feature-gates.md)的模式应拒绝调用。需要新增宿主接口、可信文件桥或运行时库时先修订核心契约及兼容协商，不能靠业务输入或插件配置开启宿主能力。

已有24个静态业务Skill及网页搜索动态微候选机制保持各自业务契约。37个规划能力不增加业务HTTP/RPC路径；2.8.0另为微部署管理新增PUT settings。未来真实发布按[实施顺序](../../preset-plugins/planned/delivery.md)与版本化变更规则执行。
